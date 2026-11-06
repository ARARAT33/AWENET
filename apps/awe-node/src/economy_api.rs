//! AWENET economy product API: AWENETwork, founder control, storefront and
//! escrowed store orders, served by the local node UI.
//!
//! This layer wires the existing deterministic core economy modules
//! (`founder`, `awenetwork`, `price_control`, `treasury`, `storefront`,
//! `store_escrow` and the ONECOIN ledger) into the product HTTP surface at
//! `127.0.0.1:41800`. The whole economy state persists in
//! `awenet-economy.json` next to the ONECOIN ledger, so tasks, prices,
//! catalog and escrow orders survive node restarts.
//!
//! Trust model, honestly stated:
//!
//! - The local node's identity is the founder at economy genesis. The
//!   founder-only endpoints (`/api/founder/*`) are additionally gated by
//!   `FounderAuthority::is_founder` against the node key, and the UI
//!   listener binds to loopback only. The browser WebAuthn ceremony for
//!   `founder_guard.rs` is still a client-side task and is not implemented
//!   in this layer.
//! - ONECOIN payouts happen only through the founder-approved, idempotent
//!   mint inside `awenetwork.rs`; this API never mints directly.
//! - Store revenue is always a buyer-to-seller transfer through escrow,
//!   never new issuance; the treasury records every flow separately.
//! - The store escrow operator is a dedicated persistent marketplace
//!   account, so buyer, seller and operator are always three distinct
//!   accounts.

use crate::parse_onecoin_atoms;
use awep2p_core::awenetwork::{
    Complexity, SubmissionStatus, TaskFilter, TaskPhase, TaskSubmission, WorkCategory, WorkMarket,
    WorkTask, WorkTaskSpec, WorkTaskStatus,
};
use awep2p_core::founder::FounderAuthority;
use awep2p_core::identity::{AweId, Identity, Username};
use awep2p_core::onecoin::{OnecoinLedger, ATOMS_PER_COIN};
use awep2p_core::price_control::{OfficialPriceControl, PRICE_DISCLAIMER};
use awep2p_core::store_escrow::{DisputeRuling, StoreEscrow, StoreOrder};
use awep2p_core::storefront::{Product, ProductFilter, ProductKind, Storefront};
use awep2p_core::treasury::{Treasury, TreasuryFlow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub type LedgerState = Arc<Mutex<OnecoinLedger>>;
pub type EconomyState = Arc<Mutex<NodeEconomy>>;
pub type ApiResponse = (&'static str, &'static str, String);

pub const JSON_MIME: &str = "application/json; charset=utf-8";
const DEFAULT_STORE_FEE_BPS: u16 = 100;
const DEFAULT_ORDER_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60;
const OVERVIEW_WINDOW_SECS: u64 = 30 * 24 * 60 * 60;
const OVERVIEW_TAIL: usize = 20;

/// The persistent AWENET economy state of this node.
#[derive(Clone, Serialize, Deserialize)]
pub struct NodeEconomy {
    pub authority: FounderAuthority,
    pub price_control: OfficialPriceControl,
    pub treasury: Treasury,
    pub market: WorkMarket,
    pub storefront: Storefront,
    pub store_escrow: StoreEscrow,
    pub store_operator: StoreOperatorKey,
    #[serde(default)]
    genesis_synced: bool,
}

/// The dedicated marketplace escrow operator account. Buyer, seller and
/// operator must be three different accounts, so the operator is not the
/// node identity. The key is persisted next to the economy state.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoreOperatorKey {
    pub username: String,
    pub secret_hex: String,
}

impl StoreOperatorKey {
    fn generate() -> Self {
        let username = Username::new("awenet-store").expect("valid store operator username");
        let identity = Identity::generate(username);
        Self {
            username: identity.public.username.as_str().to_string(),
            secret_hex: hex::encode(identity.export_secret()),
        }
    }

    pub fn identity(&self) -> Result<Identity, String> {
        let username = Username::new(&self.username).map_err(|e| e.to_string())?;
        let secret = hex::decode(&self.secret_hex)
            .map_err(|_| "store operator key is corrupt".to_string())?;
        if secret.len() != 32 {
            return Err("store operator key is corrupt".to_string());
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&secret);
        Ok(Identity::from_secret(username, bytes))
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Loads the persistent economy, or creates it on first run. The returned
/// flag tells the caller whether this machine just became the founder.
pub fn load_or_create(identity: &Identity, path: &Path) -> (NodeEconomy, bool) {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(economy) = serde_json::from_slice::<NodeEconomy>(&bytes) {
            return (economy, false);
        }
    }
    let now = now_unix();
    let operator = StoreOperatorKey::generate();
    let operator_identity = operator
        .identity()
        .expect("a freshly generated store operator key is valid");
    let store_escrow = StoreEscrow::new(&operator_identity, 10_000, DEFAULT_STORE_FEE_BPS)
        .expect("store escrow configuration is valid");
    let economy = NodeEconomy {
        authority: FounderAuthority::install(identity, now),
        price_control: OfficialPriceControl::new(100).expect("initial price is valid"),
        treasury: Treasury::new(),
        market: WorkMarket::new(),
        storefront: Storefront::new(),
        store_escrow,
        store_operator: operator,
        genesis_synced: false,
    };
    (economy, true)
}

/// Keeps the treasury issuance total equal to the real ledger issuance
/// total, so `treasury.reconcile` never reports fake reserves. Genesis and
/// join distributions minted outside this API are recorded here.
fn sync_treasury_issuance(economy: &mut NodeEconomy, ledger: &OnecoinLedger, now: u64) {
    let issued = economy.treasury.summary().total_issued_atoms;
    if ledger.total_issued_atoms > issued {
        let delta = ledger.total_issued_atoms - issued;
        let flow = if economy.genesis_synced {
            TreasuryFlow::JoinDistributionIssued
        } else {
            TreasuryFlow::GenesisIssued
        };
        if economy.treasury.record(flow, delta, now).is_ok() {
            economy.genesis_synced = true;
        }
    } else {
        economy.genesis_synced = true;
    }
}

fn save_economy(economy: &NodeEconomy, path: &Path) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(economy).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn save_ledger(ledger: &OnecoinLedger, path: &Path) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(ledger).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn ok(value: serde_json::Value) -> ApiResponse {
    ("200 OK", JSON_MIME, value.to_string())
}

fn err(status: u16, message: String) -> ApiResponse {
    let code = match status {
        403 => "403 Forbidden",
        404 => "404 Not Found",
        500 => "500 Internal Server Error",
        _ => "400 Bad Request",
    };
    (
        code,
        JSON_MIME,
        serde_json::json!({ "status": "error", "error": message }).to_string(),
    )
}

fn is_founder(economy: &NodeEconomy, identity: &Identity) -> bool {
    economy.authority.is_founder(&identity.public.public_key)
}

fn require_founder(economy: &NodeEconomy, identity: &Identity) -> Option<ApiResponse> {
    if is_founder(economy, identity) {
        None
    } else {
        Some(err(
            403,
            "only the AWENET founder may perform this operation".to_string(),
        ))
    }
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((url_decode(key), url_decode(value)))
        })
        .collect()
}

fn category_str(category: WorkCategory) -> &'static str {
    match category {
        WorkCategory::Engineering => "engineering",
        WorkCategory::Design => "design",
        WorkCategory::Content => "content",
        WorkCategory::Network => "network",
        WorkCategory::Community => "community",
    }
}

fn parse_category(value: &str) -> Result<WorkCategory, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "engineering" => Ok(WorkCategory::Engineering),
        "design" => Ok(WorkCategory::Design),
        "content" => Ok(WorkCategory::Content),
        "network" => Ok(WorkCategory::Network),
        "community" => Ok(WorkCategory::Community),
        other => Err(format!("unknown task category: {other}")),
    }
}

fn complexity_str(complexity: Complexity) -> &'static str {
    match complexity {
        Complexity::Beginner => "beginner",
        Complexity::Intermediate => "intermediate",
        Complexity::Advanced => "advanced",
        Complexity::Expert => "expert",
    }
}

fn parse_complexity(value: &str) -> Result<Complexity, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "beginner" => Ok(Complexity::Beginner),
        "intermediate" => Ok(Complexity::Intermediate),
        "advanced" => Ok(Complexity::Advanced),
        "expert" => Ok(Complexity::Expert),
        other => Err(format!("unknown complexity: {other}")),
    }
}

fn task_status_str(status: WorkTaskStatus) -> &'static str {
    match status {
        WorkTaskStatus::Draft => "draft",
        WorkTaskStatus::Published => "published",
        WorkTaskStatus::Closed => "closed",
        WorkTaskStatus::Cancelled => "cancelled",
    }
}

fn parse_task_status(value: &str) -> Result<WorkTaskStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "draft" => Ok(WorkTaskStatus::Draft),
        "published" => Ok(WorkTaskStatus::Published),
        "closed" => Ok(WorkTaskStatus::Closed),
        "cancelled" => Ok(WorkTaskStatus::Cancelled),
        other => Err(format!("unknown task status: {other}")),
    }
}

fn submission_status_str(status: SubmissionStatus) -> &'static str {
    match status {
        SubmissionStatus::Submitted => "submitted",
        SubmissionStatus::Approved => "approved",
        SubmissionStatus::Rejected => "rejected",
        SubmissionStatus::Revising => "revising",
    }
}

fn money_json(atoms: u128) -> serde_json::Value {
    serde_json::json!({
        "atoms": atoms,
        "coins": atoms / ATOMS_PER_COIN,
    })
}

fn task_json(task: &WorkTask, assignments: usize) -> serde_json::Value {
    serde_json::json!({
        "id": task.id,
        "title": task.spec.title,
        "description": task.spec.description,
        "category": category_str(task.spec.category),
        "complexity": complexity_str(task.spec.complexity),
        "deadline_unix": task.spec.deadline_unix,
        "required_skills": task.spec.required_skills,
        "expected_outcome": task.spec.expected_outcome,
        "acceptance_criteria": task.spec.acceptance_criteria,
        "reward_atoms": task.spec.reward_atoms,
        "reward_coins": task.spec.reward_atoms / ATOMS_PER_COIN,
        "phases": task.spec.phases.iter().map(|phase| serde_json::json!({
            "name": phase.name,
            "acceptance_criteria": phase.acceptance_criteria,
            "reward_atoms": phase.reward_atoms,
            "reward_coins": phase.reward_atoms / ATOMS_PER_COIN,
        })).collect::<Vec<_>>(),
        "team_allowed": task.spec.team_allowed,
        "submission_format": task.spec.submission_format,
        "status": task_status_str(task.status),
        "published_at_unix": task.published_at_unix,
        "author": task.author.to_hex(),
        "contributors": assignments,
    })
}

fn submission_json(submission: &TaskSubmission, market: &WorkMarket) -> serde_json::Value {
    let task_title = market
        .task(submission.task_id)
        .map(|task| task.spec.title.clone());
    serde_json::json!({
        "id": submission.id,
        "task_id": submission.task_id,
        "task_title": task_title,
        "contributor": submission.contributor.to_hex(),
        "status": submission_status_str(submission.status),
        "submitted_at_unix": submission.submitted_at_unix,
        "evidence": submission.evidence,
        "note": submission.note,
        "paid_atoms": submission.paid_atoms,
        "paid_coins": submission.paid_atoms / ATOMS_PER_COIN,
        "review": submission.review.as_ref().map(|review| serde_json::json!({
            "approved": review.approved,
            "reason": review.reason,
            "reviewed_at_unix": review.reviewed_at_unix,
            "reviewer": review.reviewer.to_hex(),
        })),
    })
}

fn product_json(product: &Product) -> serde_json::Value {
    serde_json::json!({
        "id": product.id,
        "seller": product.seller.to_hex(),
        "title": product.title,
        "description": product.description,
        "kind": product.kind.as_str(),
        "category": product.category,
        "price": money_json(product.price_atoms),
        "stock": product.stock,
        "sold_units": product.sold_units,
        "active": product.active,
        "listed_at_unix": product.listed_at_unix,
    })
}

fn order_json(order: &StoreOrder, status: &str, role: &str, now: u64) -> serde_json::Value {
    serde_json::json!({
        "order_id": order.escrow_id,
        "title": order.title,
        "buyer": order.buyer.to_hex(),
        "seller": order.seller.to_hex(),
        "amount": money_json(order.amount_atoms),
        "fee": money_json(order.fee_atoms),
        "created_tick": order.created_tick,
        "expiry_tick": order.expiry_tick,
        "funded": order.funded,
        "disputed": order.dispute.is_some(),
        "dispute_reason": order.dispute.as_ref().map(|dispute| dispute.reason.clone()),
        "status": status,
        "role": role,
        "now_unix": now,
    })
}

fn string_field(value: &serde_json::Value, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{name} is required"))
}

/// Reads an ONECOIN amount from a JSON object: either `atoms` as an
/// integer/string of atoms, or `coins` as a decimal coin amount.
fn amount_field(
    value: &serde_json::Value,
    coins_name: &str,
    atoms_name: &str,
) -> Result<u128, String> {
    if let Some(atoms) = value.get(atoms_name) {
        if !atoms.is_null() {
            if let Some(number) = atoms.as_u64() {
                return Ok(u128::from(number));
            }
            if let Some(text) = atoms.as_str() {
                if let Ok(parsed) = text.trim().parse::<u128>() {
                    return Ok(parsed);
                }
            }
            return Err(format!("{atoms_name} must be an integer atom amount"));
        }
    }
    if let Some(coins) = value.get(coins_name) {
        if !coins.is_null() {
            return parse_onecoin_atoms(coins);
        }
    }
    Err(format!("either {coins_name} or {atoms_name} is required"))
}

fn string_array_field(value: &serde_json::Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn spec_from_json(value: &serde_json::Value) -> Result<WorkTaskSpec, String> {
    let category = match value.get("category").and_then(serde_json::Value::as_str) {
        Some(text) => parse_category(text)?,
        None => WorkCategory::Engineering,
    };
    let complexity = match value.get("complexity").and_then(serde_json::Value::as_str) {
        Some(text) => parse_complexity(text)?,
        None => Complexity::Intermediate,
    };
    let mut phases = Vec::new();
    if let Some(items) = value.get("phases").and_then(serde_json::Value::as_array) {
        for item in items {
            phases.push(TaskPhase {
                name: string_field(item, "name")?,
                acceptance_criteria: string_field(item, "acceptance_criteria")?,
                reward_atoms: amount_field(item, "reward_coins", "reward_atoms")?,
            });
        }
    }
    Ok(WorkTaskSpec {
        title: string_field(value, "title")?,
        description: string_field(value, "description")?,
        category,
        complexity,
        deadline_unix: value
            .get("deadline_unix")
            .and_then(serde_json::Value::as_u64),
        required_skills: string_array_field(value, "required_skills")
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected_outcome: string_field(value, "expected_outcome")?,
        acceptance_criteria: string_array_field(value, "acceptance_criteria"),
        reward_atoms: amount_field(value, "reward_coins", "reward_atoms")?,
        phases,
        team_allowed: value
            .get("team_allowed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        submission_format: value
            .get("submission_format")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("link_or_attachment")
            .to_string(),
    })
}

/// Entry point used by the node UI server for economy endpoints.
#[allow(clippy::too_many_arguments)]
pub fn handle(
    method: &str,
    target: &str,
    body: &str,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    if method == "OPTIONS" {
        return ok(serde_json::json!({ "status": "ok" }));
    }
    let params = parse_query(query);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match (method, segments.as_slice()) {
        ("GET", ["api", "onecoin", "price"]) => price_public(economy_state),
        ("GET", ["api", "awenetwork", "tasks"]) => tasks_public(&params, economy_state),
        ("GET", ["api", "awenetwork", "task", id]) => match parse_u64(id) {
            Some(id) => task_public(id, economy_state),
            None => err(400, "task id must be a number".to_string()),
        },
        ("GET", ["api", "awenetwork", "profile"]) => work_profile(identity, economy_state),
        ("POST", ["api", "awenetwork", "tasks"]) => publish_task(
            &parsed,
            identity,
            ledger,
            economy_state,
            economy_path,
            ledger_path,
        ),
        ("POST", ["api", "awenetwork", "task", id, "accept"]) => match parse_u64(id) {
            Some(id) => accept_task(id, identity, economy_state, economy_path),
            None => err(400, "task id must be a number".to_string()),
        },
        ("POST", ["api", "awenetwork", "task", id, "submit"]) => match parse_u64(id) {
            Some(id) => submit_work(id, &parsed, identity, economy_state, economy_path),
            None => err(400, "task id must be a number".to_string()),
        },
        ("POST", ["api", "awenetwork", "submission", id, "resubmit"]) => match parse_u64(id) {
            Some(id) => resubmit_work(id, &parsed, identity, economy_state, economy_path),
            None => err(400, "submission id must be a number".to_string()),
        },
        ("POST", ["api", "awenetwork", "suggest"]) => {
            suggest_task(&parsed, identity, economy_state, economy_path)
        }
        ("GET", ["api", "founder", "identity"]) => founder_identity(identity, economy_state),
        ("GET", ["api", "founder", "overview"]) => founder_overview(identity, economy_state),
        ("POST", ["api", "founder", "price"]) => {
            set_official_price(&parsed, identity, economy_state, economy_path)
        }
        ("POST", ["api", "founder", "submissions", id, "approve"]) => match parse_u64(id) {
            Some(id) => approve_submission(
                id,
                identity,
                ledger,
                economy_state,
                economy_path,
                ledger_path,
            ),
            None => err(400, "submission id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "submissions", id, "approve-team"]) => match parse_u64(id) {
            Some(id) => approve_team_submission(
                id,
                &parsed,
                identity,
                ledger,
                economy_state,
                economy_path,
                ledger_path,
            ),
            None => err(400, "submission id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "submissions", id, "reject"]) => match parse_u64(id) {
            Some(id) => review_submission(
                id,
                &parsed,
                ReviewKind::Reject,
                identity,
                economy_state,
                economy_path,
            ),
            None => err(400, "submission id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "submissions", id, "revise"]) => match parse_u64(id) {
            Some(id) => review_submission(
                id,
                &parsed,
                ReviewKind::Revise,
                identity,
                economy_state,
                economy_path,
            ),
            None => err(400, "submission id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "tasks", id, "close"]) => match parse_u64(id) {
            Some(id) => close_task(id, TaskClose::Close, identity, economy_state, economy_path),
            None => err(400, "task id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "tasks", id, "cancel"]) => match parse_u64(id) {
            Some(id) => close_task(id, TaskClose::Cancel, identity, economy_state, economy_path),
            None => err(400, "task id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "suggestions", id, "approve"]) => match parse_u64(id) {
            Some(id) => approve_suggestion(id, identity, economy_state, economy_path),
            None => err(400, "suggestion id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "store", "orders", id, "resolve"]) => match parse_u64(id) {
            Some(id) => resolve_dispute(
                id,
                &parsed,
                identity,
                ledger,
                economy_state,
                economy_path,
                ledger_path,
            ),
            None => err(400, "order id must be a number".to_string()),
        },
        ("POST", ["api", "founder", "store", "orders", id, "refund"]) => match parse_u64(id) {
            Some(id) => founder_refund(
                id,
                identity,
                ledger,
                economy_state,
                economy_path,
                ledger_path,
            ),
            None => err(400, "order id must be a number".to_string()),
        },
        ("GET", ["api", "storefront", "products"]) => products_public(&params, economy_state),
        ("GET", ["api", "storefront", "seller"]) => seller_page(identity, economy_state),
        ("GET", ["api", "storefront", "orders"]) => store_orders(identity, economy_state),
        ("POST", ["api", "storefront", "products"]) => {
            list_product(&parsed, identity, economy_state, economy_path)
        }
        ("POST", ["api", "storefront", "products", id, "update"]) => match parse_u64(id) {
            Some(id) => update_product(id, &parsed, identity, economy_state, economy_path),
            None => err(400, "product id must be a number".to_string()),
        },
        ("POST", ["api", "storefront", "products", id, "deactivate"]) => match parse_u64(id) {
            Some(id) => deactivate_product(id, identity, economy_state, economy_path),
            None => err(400, "product id must be a number".to_string()),
        },
        ("POST", ["api", "storefront", "orders"]) => place_order(
            &parsed,
            identity,
            ledger,
            economy_state,
            economy_path,
            ledger_path,
        ),
        ("POST", ["api", "storefront", "orders", id, "confirm"]) => match parse_u64(id) {
            Some(id) => confirm_order(
                id,
                identity,
                ledger,
                economy_state,
                economy_path,
                ledger_path,
            ),
            None => err(400, "order id must be a number".to_string()),
        },
        ("POST", ["api", "storefront", "orders", id, "dispute"]) => match parse_u64(id) {
            Some(id) => dispute_order(id, &parsed, identity, economy_state, economy_path),
            None => err(400, "order id must be a number".to_string()),
        },
        _ => err(404, format!("unknown economy endpoint: {method} {path}")),
    }
}

fn parse_u64(value: &str) -> Option<u64> {
    value.parse::<u64>().ok()
}

enum ReviewKind {
    Reject,
    Revise,
}

enum TaskClose {
    Close,
    Cancel,
}

fn lock_economy(
    economy_state: &EconomyState,
) -> Result<std::sync::MutexGuard<'_, NodeEconomy>, ApiResponse> {
    economy_state
        .lock()
        .map_err(|_| err(500, "economy state lock failed".to_string()))
}

fn lock_ledger(
    ledger: &LedgerState,
) -> Result<std::sync::MutexGuard<'_, OnecoinLedger>, ApiResponse> {
    ledger
        .lock()
        .map_err(|_| err(500, "ONECOIN ledger lock failed".to_string()))
}

// ---------------------------------------------------------------------------
// Public reads
// ---------------------------------------------------------------------------

fn price_public(economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    ok(serde_json::json!({
        "status": "ok",
        "official_price_usd_cents": economy.price_control.price_at(now),
        "current_price_usd_cents": economy.price_control.current_price(),
        "changed_by": "founder",
        "disclaimer": PRICE_DISCLAIMER,
        "history": economy.price_control.history().iter().map(|change| serde_json::json!({
            "sequence": change.sequence,
            "old_price_usd_cents": change.old_price_usd_cents,
            "new_price_usd_cents": change.new_price_usd_cents,
            "effective_at_unix": change.effective_at_unix,
            "recorded_at_unix": change.recorded_at_unix,
            "reason": change.reason,
            "changed_by": change.changed_by.to_hex(),
        })).collect::<Vec<_>>(),
    }))
}

fn tasks_public(params: &BTreeMap<String, String>, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let filter = TaskFilter {
        category: match params.get("category") {
            Some(text) => match parse_category(text) {
                Ok(category) => Some(category),
                Err(message) => return err(400, message),
            },
            None => None,
        },
        complexity: match params.get("complexity") {
            Some(text) => match parse_complexity(text) {
                Ok(complexity) => Some(complexity),
                Err(message) => return err(400, message),
            },
            None => None,
        },
        status: match params.get("status") {
            Some(text) => match parse_task_status(text) {
                Ok(status) => Some(status),
                Err(message) => return err(400, message),
            },
            None => Some(WorkTaskStatus::Published),
        },
        min_reward_atoms: match params.get("min_reward_coins") {
            Some(text) => match parse_onecoin_atoms(&serde_json::Value::String(text.clone())) {
                Ok(atoms) => atoms,
                Err(_) => return err(400, "min_reward_coins must be a decimal amount".to_string()),
            },
            None => params
                .get("min_reward_atoms")
                .and_then(|text| text.parse::<u128>().ok())
                .unwrap_or(0),
        },
    };
    let tasks = economy.market.find_tasks(&filter);
    let items: Vec<serde_json::Value> = tasks
        .iter()
        .map(|task| task_json(task, economy.market.assignment_count(task.id)))
        .collect();
    ok(serde_json::json!({ "status": "ok", "tasks": items, "count": items.len() }))
}

fn task_public(task_id: u64, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    match economy.market.task(task_id) {
        Some(task) => ok(serde_json::json!({
            "status": "ok",
            "task": task_json(task, economy.market.assignment_count(task_id)),
        })),
        None => err(404, format!("task {task_id} is unknown")),
    }
}

fn work_profile(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let me = &identity.public.awe_id;
    let profile = economy.market.contributor_profile(me);
    let level = economy.market.contributor_level(me);
    let submissions: Vec<serde_json::Value> = economy
        .market
        .submissions()
        .filter(|submission| submission.contributor == *me)
        .map(|submission| submission_json(submission, &economy.market))
        .collect();
    ok(serde_json::json!({
        "status": "ok",
        "awe_id": me.to_hex(),
        "accepted_tasks": profile.accepted_tasks,
        "approved": profile.approved,
        "rejected": profile.rejected,
        "revising": profile.revising,
        "earned": money_json(profile.earned_atoms),
        "level": level.as_str(),
        "submissions": submissions,
    }))
}

// ---------------------------------------------------------------------------
// AWENETwork participant actions
// ---------------------------------------------------------------------------

fn publish_task(
    parsed: &serde_json::Value,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let spec = match spec_from_json(parsed) {
        Ok(spec) => spec,
        Err(message) => return err(400, message),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    match economy.market.publish_task(
        &mut economy.authority,
        &identity.public.public_key,
        spec,
        now,
    ) {
        Ok(task_id) => {
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "published", "task_id": task_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn accept_task(
    task_id: u64,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let me = &identity.public.awe_id;
    match economy.market.accept_task(me, task_id, now_unix()) {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "accepted", "task_id": task_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn submit_work(
    task_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let evidence = string_array_field(parsed, "evidence");
    let note = parsed
        .get("note")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|note| !note.trim().is_empty());
    match economy
        .market
        .submit_work(&identity.public.awe_id, task_id, evidence, note, now_unix())
    {
        Ok(submission_id) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "submitted",
                "task_id": task_id,
                "submission_id": submission_id,
                "review": "awaiting founder review",
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn resubmit_work(
    submission_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let evidence = string_array_field(parsed, "evidence");
    match economy
        .market
        .resubmit(&identity.public.awe_id, submission_id, evidence, now_unix())
    {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "resubmitted", "submission_id": submission_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn suggest_task(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let spec = match spec_from_json(parsed) {
        Ok(spec) => spec,
        Err(message) => return err(400, message),
    };
    match economy
        .market
        .suggest_task(&identity.public.awe_id, spec, now_unix())
    {
        Ok(suggestion_id) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "suggested",
                "suggestion_id": suggestion_id,
                "official": false,
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Founder control center
// ---------------------------------------------------------------------------

fn founder_identity(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    ok(serde_json::json!({
        "status": "ok",
        "founder": is_founder(&economy, identity),
        "me": identity.public.awe_id.to_hex(),
        "founder_awe_id": economy.authority.founder_awe_id().to_hex(),
    }))
}

fn founder_overview(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let now = now_unix();
    let window_start = now.saturating_sub(OVERVIEW_WINDOW_SECS);
    let summary = economy.treasury.summary();
    let tasks_by_status = |status: WorkTaskStatus| {
        economy
            .market
            .find_tasks(&TaskFilter {
                status: Some(status),
                ..Default::default()
            })
            .len()
    };
    let pending_submissions: Vec<serde_json::Value> = economy
        .market
        .submissions()
        .filter(|submission| {
            matches!(
                submission.status,
                SubmissionStatus::Submitted | SubmissionStatus::Revising
            )
        })
        .map(|submission| submission_json(submission, &economy.market))
        .collect();
    let pending_suggestions: Vec<serde_json::Value> = economy
        .market
        .suggestions()
        .filter(|suggestion| !suggestion.approved)
        .map(|suggestion| {
            serde_json::json!({
                "id": suggestion.id,
                "contributor": suggestion.contributor.to_hex(),
                "title": suggestion.spec.title,
                "reward_atoms": suggestion.spec.reward_atoms,
                "suggested_at_unix": suggestion.suggested_at_unix,
            })
        })
        .collect();
    let orders: Vec<serde_json::Value> = economy
        .store_escrow
        .orders()
        .filter_map(|order| {
            let status = economy.store_escrow.status(order.escrow_id, now).ok()?;
            let status = if order.dispute.is_some() {
                "disputed".to_string()
            } else {
                status.as_str().to_string()
            };
            Some(serde_json::json!({
                "order_id": order.escrow_id,
                "title": order.title,
                "buyer": order.buyer.to_hex(),
                "seller": order.seller.to_hex(),
                "amount": money_json(order.amount_atoms),
                "fee": money_json(order.fee_atoms),
                "funded": order.funded,
                "disputed": order.dispute.is_some(),
                "status": status,
            }))
        })
        .collect();
    let log_tail: Vec<serde_json::Value> = economy
        .authority
        .log()
        .iter()
        .rev()
        .take(OVERVIEW_TAIL)
        .map(|entry| {
            serde_json::json!({
                "sequence": entry.sequence,
                "action": entry.action.as_str(),
                "actor": entry.actor.to_hex(),
                "at_unix": entry.at_unix,
                "detail": entry.detail,
            })
        })
        .collect();
    ok(serde_json::json!({
        "status": "ok",
        "founder": true,
        "price": {
            "official_price_usd_cents": economy.price_control.price_at(now),
            "disclaimer": PRICE_DISCLAIMER,
        },
        "treasury": {
            "total_issued": money_json(summary.total_issued_atoms),
            "circulating": money_json(summary.circulating_atoms),
            "escrow_held": money_json(summary.escrow_held_atoms),
            "task_rewards": money_json(summary.task_reward_atoms),
            "resource_rewards": money_json(summary.resource_reward_atoms),
            "store_volume": money_json(summary.store_volume_atoms),
            "seller_income": money_json(summary.seller_income_atoms),
            "transfers": money_json(summary.transfer_volume_atoms),
            "fees": money_json(summary.fee_atoms),
            "entries": economy.treasury.entry_count(),
            "inflation_risk": format!("{:?}", economy.treasury.inflation_risk(window_start, now)),
            "month": {
                "issued": money_json(economy.treasury.period_stats(window_start, now).issued_atoms),
                "circulated": money_json(economy.treasury.period_stats(window_start, now).circulated_atoms),
            },
        },
        "tasks": {
            "published": tasks_by_status(WorkTaskStatus::Published),
            "draft": tasks_by_status(WorkTaskStatus::Draft),
            "closed": tasks_by_status(WorkTaskStatus::Closed),
            "cancelled": tasks_by_status(WorkTaskStatus::Cancelled),
            "total": economy.market.task_count(),
            "submissions": economy.market.submission_count(),
            "suggestions": economy.market.suggestion_count(),
        },
        "store": {
            "products": economy.storefront.product_count(),
            "orders": orders,
            "fee_bps": economy.store_escrow.fee_bps(),
        },
        "pending_submissions": pending_submissions,
        "pending_suggestions": pending_suggestions,
        "founder_log": log_tail,
    }))
}

fn set_official_price(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let new_price = match parsed
        .get("new_price_usd_cents")
        .and_then(serde_json::Value::as_u64)
    {
        Some(cents) => cents,
        None => return err(400, "new_price_usd_cents is required".to_string()),
    };
    let reason = match parsed.get("reason").and_then(serde_json::Value::as_str) {
        Some(reason) => reason.trim().to_string(),
        None => return err(400, "a public reason is required".to_string()),
    };
    let now = now_unix();
    let effective_at = parsed
        .get("effective_at_unix")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(now);
    match economy.price_control.set_price(
        &mut economy.authority,
        &identity.public.public_key,
        new_price,
        effective_at,
        now,
        &reason,
    ) {
        Ok(change) => {
            let response = serde_json::json!({
                "status": "changed",
                "sequence": change.sequence,
                "old_price_usd_cents": change.old_price_usd_cents,
                "new_price_usd_cents": change.new_price_usd_cents,
                "effective_at_unix": change.effective_at_unix,
                "reason": change.reason,
            });
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(response)
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn approve_submission(
    submission_id: u64,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let contributor = match economy.market.submission(submission_id) {
        Some(submission) => submission.contributor.clone(),
        None => return err(404, format!("submission {submission_id} is unknown")),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    ledger.ensure_member(&contributor);
    match economy.market.approve_and_pay(
        &mut economy.authority,
        identity,
        &mut ledger,
        submission_id,
        now,
    ) {
        Ok(receipt) => {
            let _ =
                economy
                    .treasury
                    .record(TreasuryFlow::TaskRewardIssued, receipt.paid_atoms, now);
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "approved",
                "task_id": receipt.task_id,
                "submission_id": receipt.submission_id,
                "contributor": receipt.contributor.to_hex(),
                "paid": money_json(receipt.paid_atoms),
                "reward_id": hex::encode(receipt.reward_id),
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn approve_team_submission(
    submission_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let mut split: Vec<(AweId, u128)> = Vec::new();
    if let Some(items) = parsed.get("split").and_then(serde_json::Value::as_array) {
        for item in items {
            let awe_id = match item.get("awe_id").and_then(serde_json::Value::as_str) {
                Some(text) => match AweId::from_hex(text.trim()) {
                    Ok(id) => id,
                    Err(message) => return err(400, message),
                },
                None => return err(400, "each split entry needs an awe_id".to_string()),
            };
            let atoms = match amount_field(item, "coins", "atoms") {
                Ok(atoms) => atoms,
                Err(message) => return err(400, message),
            };
            split.push((awe_id, atoms));
        }
    }
    if split.is_empty() {
        return err(400, "split is required".to_string());
    }
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    for (recipient, _) in &split {
        ledger.ensure_member(recipient);
    }
    match economy.market.approve_and_pay_team(
        &mut economy.authority,
        identity,
        &mut ledger,
        submission_id,
        &split,
        now,
    ) {
        Ok(receipt) => {
            let _ =
                economy
                    .treasury
                    .record(TreasuryFlow::TaskRewardIssued, receipt.paid_atoms, now);
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            let recipients: Vec<serde_json::Value> = receipt
                .recipients
                .iter()
                .map(|(recipient, atoms)| {
                    serde_json::json!({
                        "awe_id": recipient.to_hex(),
                        "paid": money_json(*atoms),
                    })
                })
                .collect();
            ok(serde_json::json!({
                "status": "approved",
                "task_id": receipt.task_id,
                "submission_id": receipt.submission_id,
                "recipients": recipients,
                "paid": money_json(receipt.paid_atoms),
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn review_submission(
    submission_id: u64,
    parsed: &serde_json::Value,
    kind: ReviewKind,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let reason = match parsed.get("reason").and_then(serde_json::Value::as_str) {
        Some(reason) => reason.trim().to_string(),
        None => return err(400, "a reason is required".to_string()),
    };
    let now = now_unix();
    let result = match kind {
        ReviewKind::Reject => economy.market.reject_submission(
            &mut economy.authority,
            &identity.public.public_key,
            submission_id,
            &reason,
            now,
        ),
        ReviewKind::Revise => economy.market.request_revision(
            &mut economy.authority,
            &identity.public.public_key,
            submission_id,
            &reason,
            now,
        ),
    };
    match result {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            let status = match kind {
                ReviewKind::Reject => "rejected",
                ReviewKind::Revise => "revising",
            };
            ok(serde_json::json!({
                "status": status,
                "submission_id": submission_id,
                "reason": reason,
                "reward_minted": 0,
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn close_task(
    task_id: u64,
    kind: TaskClose,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let now = now_unix();
    let result = match kind {
        TaskClose::Close => economy.market.close_task(
            &mut economy.authority,
            &identity.public.public_key,
            task_id,
            now,
        ),
        TaskClose::Cancel => economy.market.cancel_task(
            &mut economy.authority,
            &identity.public.public_key,
            task_id,
            now,
        ),
    };
    match result {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            let status = match kind {
                TaskClose::Close => "closed",
                TaskClose::Cancel => "cancelled",
            };
            ok(serde_json::json!({ "status": status, "task_id": task_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn approve_suggestion(
    suggestion_id: u64,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    match economy.market.approve_suggestion(
        &mut economy.authority,
        &identity.public.public_key,
        suggestion_id,
        now_unix(),
    ) {
        Ok(task_id) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "published",
                "task_id": task_id,
                "suggestion_id": suggestion_id,
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn resolve_dispute(
    order_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let ruling = match parsed.get("ruling").and_then(serde_json::Value::as_str) {
        Some("refund_buyer") => DisputeRuling::RefundBuyer,
        Some("release_seller") => DisputeRuling::ReleaseSeller,
        _ => {
            return err(
                400,
                "ruling must be refund_buyer or release_seller".to_string(),
            )
        }
    };
    let (amount_atoms, fee_atoms) = match economy.store_escrow.order(order_id) {
        Some(order) => (order.amount_atoms, order.fee_atoms),
        None => return err(404, format!("order {order_id} is unknown")),
    };
    let operator = match economy.store_operator.identity() {
        Ok(operator) => operator,
        Err(message) => return err(500, message),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    let seller = match economy.store_escrow.order(order_id) {
        Some(order) => order.seller.clone(),
        None => return err(404, format!("order {order_id} is unknown")),
    };
    ledger.ensure_member(&seller);
    match economy.store_escrow.resolve_dispute(
        &mut economy.authority,
        &identity.public.public_key,
        &mut ledger,
        &operator,
        order_id,
        ruling,
        now,
    ) {
        Ok(settled) => {
            match ruling {
                DisputeRuling::RefundBuyer => {
                    let _ = economy
                        .treasury
                        .record(TreasuryFlow::EscrowRefund, amount_atoms, now);
                }
                DisputeRuling::ReleaseSeller => {
                    let _ = economy
                        .treasury
                        .record(TreasuryFlow::EscrowRelease, amount_atoms, now);
                    let net = amount_atoms.saturating_sub(fee_atoms);
                    if net > 0 {
                        let _ = economy
                            .treasury
                            .record(TreasuryFlow::SellerIncome, net, now);
                    }
                    if fee_atoms > 0 {
                        let _ =
                            economy
                                .treasury
                                .record(TreasuryFlow::MarketplaceFee, fee_atoms, now);
                    }
                }
            }
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "settled",
                "order_id": order_id,
                "settled": money_json(settled),
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn founder_refund(
    order_id: u64,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    if economy.store_escrow.order(order_id).is_none() {
        return err(404, format!("order {order_id} is unknown"));
    }
    let operator = match economy.store_operator.identity() {
        Ok(operator) => operator,
        Err(message) => return err(500, message),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    match economy
        .store_escrow
        .refund(&mut ledger, &operator, order_id)
    {
        Ok(refunded) => {
            let _ = economy
                .treasury
                .record(TreasuryFlow::EscrowRefund, refunded, now);
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "refunded",
                "order_id": order_id,
                "refunded": money_json(refunded),
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Storefront
// ---------------------------------------------------------------------------

fn products_public(params: &BTreeMap<String, String>, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let kind = match params
        .get("kind")
        .map(|text| ProductKind::parse(text).ok_or_else(|| format!("unknown product kind: {text}")))
    {
        Some(Ok(kind)) => Some(kind),
        Some(Err(message)) => return err(400, message),
        None => None,
    };
    let filter = ProductFilter {
        query: params.get("q").cloned().unwrap_or_default(),
        kind,
        category: params.get("category").cloned(),
        seller: None,
        min_price_atoms: params
            .get("min_price_atoms")
            .and_then(|text| text.parse::<u128>().ok())
            .unwrap_or(0),
        max_price_atoms: params
            .get("max_price_atoms")
            .and_then(|text| text.parse::<u128>().ok()),
        include_inactive: false,
    };
    let products: Vec<serde_json::Value> = economy
        .storefront
        .search(&filter)
        .iter()
        .map(|product| product_json(product))
        .collect();
    ok(serde_json::json!({ "status": "ok", "products": products, "count": products.len() }))
}

fn seller_page(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let me = &identity.public.awe_id;
    let stats = economy.storefront.seller_stats(me);
    let products: Vec<serde_json::Value> = economy
        .storefront
        .seller_products(me)
        .iter()
        .map(|product| product_json(product))
        .collect();
    ok(serde_json::json!({
        "status": "ok",
        "seller": me.to_hex(),
        "products": products,
        "stats": {
            "products": stats.products,
            "active_products": stats.active_products,
            "sold_units": stats.sold_units,
        },
    }))
}

fn list_product(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let title = match string_field(parsed, "title") {
        Ok(title) => title,
        Err(message) => return err(400, message),
    };
    let description = parsed
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let kind = match parsed.get("kind").and_then(serde_json::Value::as_str) {
        Some(text) => match ProductKind::parse(text) {
            Some(kind) => kind,
            None => return err(400, format!("unknown product kind: {text}")),
        },
        None => return err(400, "kind must be digital, service or physical".to_string()),
    };
    let category = parsed
        .get("category")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let price_atoms = match amount_field(parsed, "price_coins", "price_atoms") {
        Ok(atoms) => atoms,
        Err(message) => return err(400, message),
    };
    let stock = parsed.get("stock").and_then(serde_json::Value::as_u64);
    match economy.storefront.list_product(
        &identity.public.awe_id,
        &title,
        description,
        kind,
        category,
        price_atoms,
        stock,
        now_unix(),
    ) {
        Ok(product_id) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "listed", "product_id": product_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn update_product(
    product_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let title = match string_field(parsed, "title") {
        Ok(title) => title,
        Err(message) => return err(400, message),
    };
    let description = parsed
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let category = parsed
        .get("category")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let price_atoms = match amount_field(parsed, "price_coins", "price_atoms") {
        Ok(atoms) => atoms,
        Err(message) => return err(400, message),
    };
    let stock = parsed.get("stock").and_then(serde_json::Value::as_u64);
    let active = parsed
        .get("active")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    match economy.storefront.update_product(
        &identity.public.awe_id,
        product_id,
        &title,
        description,
        category,
        price_atoms,
        stock,
        active,
    ) {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "updated", "product_id": product_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn deactivate_product(
    product_id: u64,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    match economy
        .storefront
        .deactivate_product(&identity.public.awe_id, product_id)
    {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({ "status": "deactivated", "product_id": product_id }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn place_order(
    parsed: &serde_json::Value,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let product_id = match parsed.get("product_id").and_then(serde_json::Value::as_u64) {
        Some(id) => id,
        None => return err(400, "product_id is required".to_string()),
    };
    let units = match parsed.get("units").and_then(serde_json::Value::as_u64) {
        Some(units) if units > 0 => units,
        _ => return err(400, "units must be a positive number".to_string()),
    };
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let product = match economy.storefront.product(product_id) {
        Some(product) => product.clone(),
        None => return err(404, format!("product {product_id} is unknown")),
    };
    let me = identity.public.awe_id.clone();
    if product.seller == me {
        return err(
            400,
            format!("product {product_id} is your own listing; buyer and seller must differ"),
        );
    }
    if !product.active {
        return err(400, format!("product {product_id} is not active"));
    }
    let amount_atoms = product.price_atoms.saturating_mul(u128::from(units));
    if amount_atoms == 0 {
        return err(400, "order amount must be positive".to_string());
    }
    let now = now_unix();
    let expiry = parsed
        .get("expiry_unix")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| now.saturating_add(DEFAULT_ORDER_EXPIRY_SECS));
    let operator = match economy.store_operator.identity() {
        Ok(operator) => operator,
        Err(message) => return err(500, message),
    };
    let order_id = match economy.store_escrow.open_order(
        &me,
        &product.seller,
        amount_atoms,
        &product.title,
        now,
        Some(expiry),
    ) {
        Ok(order_id) => order_id,
        Err(message) => return err(400, message.to_string()),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    sync_treasury_issuance(&mut economy, &ledger, now);
    ledger.ensure_member(&operator.public.awe_id);
    match economy
        .store_escrow
        .fund(&mut ledger, identity, &operator, order_id, now)
    {
        Ok(()) => {
            if let Err(message) = economy.storefront.record_sale(&me, product_id, units) {
                return err(500, format!("catalog sale recording failed: {message}"));
            }
            let _ = economy
                .treasury
                .record(TreasuryFlow::StorePurchase, amount_atoms, now);
            let _ = economy
                .treasury
                .record(TreasuryFlow::EscrowHold, amount_atoms, now);
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "funded",
                "order_id": order_id,
                "product_id": product_id,
                "amount": money_json(amount_atoms),
                "escrow": "held until delivery confirmation",
            }))
        }
        Err(message) => {
            // The unfunded order stays in Created state awaiting payment and
            // expires harmlessly; no stock was consumed and no funds moved.
            // Persist it so memory and disk stay consistent.
            let _ = save_economy(&economy, economy_path);
            err(400, message.to_string())
        }
    }
}

fn store_orders(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let me = &identity.public.awe_id;
    let now = now_unix();
    let orders: Vec<serde_json::Value> = economy
        .store_escrow
        .orders()
        .filter(|order| order.buyer == *me || order.seller == *me)
        .filter_map(|order| {
            let status = economy
                .store_escrow
                .status(order.escrow_id, now)
                .ok()?
                .as_str()
                .to_string();
            let role = if order.buyer == *me {
                "buyer"
            } else {
                "seller"
            };
            Some(order_json(order, &status, role, now))
        })
        .collect();
    ok(serde_json::json!({ "status": "ok", "orders": orders, "count": orders.len() }))
}

fn confirm_order(
    order_id: u64,
    identity: &Identity,
    ledger: &LedgerState,
    economy_state: &EconomyState,
    economy_path: &Path,
    ledger_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let (amount_atoms, fee_atoms, seller) = match economy.store_escrow.order(order_id) {
        Some(order) => (order.amount_atoms, order.fee_atoms, order.seller.clone()),
        None => return err(404, format!("order {order_id} is unknown")),
    };
    let operator = match economy.store_operator.identity() {
        Ok(operator) => operator,
        Err(message) => return err(500, message),
    };
    let mut ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    ledger.ensure_member(&seller);
    match economy.store_escrow.confirm_delivery(
        &mut ledger,
        &operator,
        &identity.public.awe_id,
        order_id,
    ) {
        Ok(net_atoms) => {
            let _ = economy
                .treasury
                .record(TreasuryFlow::EscrowRelease, amount_atoms, now);
            if net_atoms > 0 {
                let _ = economy
                    .treasury
                    .record(TreasuryFlow::SellerIncome, net_atoms, now);
            }
            if fee_atoms > 0 {
                let _ = economy
                    .treasury
                    .record(TreasuryFlow::MarketplaceFee, fee_atoms, now);
            }
            if let Err(message) = save_ledger(&ledger, ledger_path) {
                return err(500, message);
            }
            drop(ledger);
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "released",
                "order_id": order_id,
                "seller_paid": money_json(net_atoms),
                "fee": money_json(fee_atoms),
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

fn dispute_order(
    order_id: u64,
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let reason = match parsed.get("reason").and_then(serde_json::Value::as_str) {
        Some(reason) => reason.trim().to_string(),
        None => return err(400, "a dispute reason is required".to_string()),
    };
    match economy
        .store_escrow
        .dispute(&identity.public.awe_id, order_id, &reason, now_unix())
    {
        Ok(()) => {
            if let Err(message) = save_economy(&economy, economy_path) {
                return err(500, message);
            }
            ok(serde_json::json!({
                "status": "disputed",
                "order_id": order_id,
                "resolution": "founder-only",
            }))
        }
        Err(message) => err(400, message.to_string()),
    }
}

/// Periodic sweep that expires held store orders and refunds the buyers.
/// Called from the node runtime; every refund is recorded in the treasury.
pub fn sweep_expired_orders(
    economy_state: &EconomyState,
    ledger: &LedgerState,
    economy_path: &Path,
    ledger_path: &Path,
) {
    let Ok(mut economy) = economy_state.lock() else {
        return;
    };
    let Ok(mut ledger) = ledger.lock() else {
        return;
    };
    let Ok(operator) = economy.store_operator.identity() else {
        return;
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    match economy.store_escrow.expire(&mut ledger, &operator, now) {
        Ok(refunded) if !refunded.is_empty() => {
            for order_id in refunded {
                if let Some(order) = economy.store_escrow.order(order_id) {
                    if order.amount_atoms > 0 {
                        let _ = economy.treasury.record(
                            TreasuryFlow::EscrowRefund,
                            order.amount_atoms,
                            now,
                        );
                    }
                }
            }
            if save_ledger(&ledger, ledger_path).is_ok() {
                let _ = save_economy(&economy, economy_path);
            }
        }
        Ok(_) => {}
        Err(_) => {}
    }
}
