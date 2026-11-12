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
//!   listener binds to loopback only. Once a passkey is enrolled through
//!   the browser WebAuthn ceremony below, every mutating founder endpoint
//!   additionally requires a live MFA step-up session, so the founder key
//!   alone can no longer publish tasks, approve rewards or change prices.
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
use awep2p_core::founder_guard::FounderGuard;
use awep2p_core::i18n::{product_catalog, Locale, FALLBACK_LOCALE};
use awep2p_core::identity::{secure_random, AweId, Identity, Username};
use awep2p_core::marketplace_sync::RemoteCatalog;
use awep2p_core::onecoin::{OnecoinLedger, ATOMS_PER_COIN};
use awep2p_core::price_control::{OfficialPriceControl, PRICE_DISCLAIMER};
use awep2p_core::resource_economy::MeasuredUsage;
use awep2p_core::resource_meter::SignedUsageReport;
use awep2p_core::store_escrow::{DisputeRuling, StoreEscrow, StoreOrder};
use awep2p_core::storefront::{Product, ProductFilter, ProductKind, Storefront};
use awep2p_core::treasury::{Treasury, TreasuryFlow};
use awep2p_core::verifier_net::{self as verifier_net_store, VerifierNet};
use awep2p_core::webauthn;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub type LedgerState = Arc<Mutex<OnecoinLedger>>;
pub type EconomyState = Arc<Mutex<NodeEconomy>>;
pub type VerifierNetState = Arc<Mutex<VerifierNet>>;
pub type RemoteCatalogState = Arc<Mutex<RemoteCatalog>>;
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
    pub founder_guard: Option<FounderGuard>,
    #[serde(default)]
    passkey_ceremony: Option<PasskeyCeremony>,
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

/// Browser WebAuthn ceremonies older than this are refused.
const CEREMONY_TTL_SECS: u64 = 600;

/// The in-flight browser WebAuthn ceremony state. The challenge lives
/// server-side, is bound into clientDataJSON, and never reaches disk.
#[derive(Clone, Serialize, Deserialize)]
pub struct PasskeyCeremony {
    pub challenge: [u8; 32],
    pub step_up: bool,
    pub label: String,
    pub created_at_unix: u64,
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
        founder_guard: None,
        passkey_ceremony: None,
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
    // Step-up challenges, open MFA sessions and browser ceremonies are
    // in-memory auth state by contract; only enrolled credentials and
    // counters ever reach the durable snapshot.
    let mut snapshot = economy.clone();
    if let Some(guard) = snapshot.founder_guard.as_mut() {
        guard.clear_transient_auth_state();
    }
    snapshot.passkey_ceremony = None;
    let bytes = serde_json::to_vec_pretty(&snapshot).map_err(|e| e.to_string())?;
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

fn ensure_founder_guard(economy: &mut NodeEconomy, identity: &Identity) {
    if economy.founder_guard.is_none() {
        economy.founder_guard = Some(FounderGuard::install(identity, now_unix()));
    }
}

/// The gate for every mutating founder endpoint: the founder-key check plus,
/// once any passkey is enrolled, a live MFA step-up session. Before the
/// first enrollment the founder key alone stands, which is the honest
/// bootstrap rule for the very first ceremony.
fn require_founder_action(economy: &NodeEconomy, identity: &Identity) -> Option<ApiResponse> {
    if let Some(response) = require_founder(economy, identity) {
        return Some(response);
    }
    let guard = economy.founder_guard.as_ref()?;
    if !guard.passkeys().is_empty() && !guard.session_active(now_unix()) {
        return Some(err(
            403,
            "founder MFA required: complete a passkey step-up first".to_string(),
        ));
    }
    None
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
    verifier_state: &VerifierNetState,
    verifier_path: &Path,
    marketplace_state: &RemoteCatalogState,
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
        ("GET", ["api", "i18n", "messages"]) => i18n_messages(&params),
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
        ("GET", ["api", "founder", "passkey", "status"]) => passkey_status(identity, economy_state),
        ("POST", ["api", "founder", "passkey", "register", "options"]) => {
            passkey_register_options(identity, economy_state, economy_path)
        }
        ("POST", ["api", "founder", "passkey", "register"]) => {
            passkey_register(&parsed, identity, economy_state, economy_path)
        }
        ("POST", ["api", "founder", "passkey", "step-up", "options"]) => {
            passkey_step_up_options(identity, economy_state, economy_path)
        }
        ("POST", ["api", "founder", "passkey", "step-up"]) => {
            passkey_step_up(&parsed, identity, economy_state, economy_path)
        }
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
        ("GET", ["api", "storefront", "network"]) => network_catalog(&params, marketplace_state),
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
        ("GET", ["api", "verifier", "net"]) => verifier_net_status(verifier_state),
        ("POST", ["api", "verifier", "reports"]) => {
            verifier_submit_report(&parsed, identity, verifier_state, verifier_path)
        }
        ("POST", ["api", "founder", "verifier", "enroll"]) => founder_enroll_verifier(
            &parsed,
            identity,
            economy_state,
            verifier_state,
            verifier_path,
        ),
        ("POST", ["api", "founder", "verifier", "retire"]) => founder_retire_verifier(
            &parsed,
            identity,
            economy_state,
            verifier_state,
            verifier_path,
        ),
        ("POST", ["api", "founder", "verifier", "window"]) => founder_open_window(
            &parsed,
            identity,
            economy_state,
            verifier_state,
            verifier_path,
        ),
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

/// Serves the bilingual economy message catalog for the desktop UI screens.
/// The catalog itself lives in `core/src/i18n.rs`; the UI loads it once per
/// locale through `GET /api/i18n/messages` and renders every economy screen
/// through `window.AWEI18N.t`.
fn i18n_messages(params: &BTreeMap<String, String>) -> ApiResponse {
    let requested = params
        .get("locale")
        .map(String::as_str)
        .unwrap_or(FALLBACK_LOCALE);
    let locale = match Locale::from_code(requested) {
        Some(locale) => locale,
        None => return err(400, "locale must be en or hy".to_string()),
    };
    let messages = product_catalog().messages(locale);
    ok(serde_json::json!({
        "status": "ok",
        "locale": locale.code(),
        "fallback_locale": FALLBACK_LOCALE,
        "message_count": messages.len(),
        "messages": messages,
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
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let spec = match spec_from_json(parsed) {
        Ok(spec) => spec,
        Err(message) => return err(400, message),
    };
    let ledger = match lock_ledger(ledger) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let now = now_unix();
    sync_treasury_issuance(&mut economy, &ledger, now);
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    match market.publish_task(authority, &identity.public.public_key, spec, now) {
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

fn passkey_status(identity: &Identity, economy_state: &EconomyState) -> ApiResponse {
    let economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder(&economy, identity) {
        return response;
    }
    let guard = economy.founder_guard.as_ref();
    ok(serde_json::json!({
        "status": "ok",
        "enrolled": guard.map(|g| g.passkeys().len()).unwrap_or(0),
        "session_active": guard.map(|g| g.session_active(now_unix())).unwrap_or(false),
        "recovery_codes_remaining": guard.map(|g| g.recovery_codes_remaining()).unwrap_or(0),
        "relying_party_id": webauthn::RP_ID,
        "passkeys": guard
            .map(|g| {
                g.passkeys()
                    .iter()
                    .map(|passkey| {
                        serde_json::json!({
                            "credential_id_hex": hex::encode(passkey.credential_id),
                            "label": passkey.label,
                            "enrolled_at_unix": passkey.enrolled_at_unix,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "session": guard
            .and_then(|g| g.session())
            .map(|session| {
                serde_json::json!({
                    "granted_at_unix": session.granted_at_unix,
                    "expires_at_unix": session.expires_at_unix,
                })
            }),
    }))
}

fn passkey_register_options(
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
    ensure_founder_guard(&mut economy, identity);
    let challenge: [u8; 32] = *secure_random::<32>();
    economy.passkey_ceremony = Some(PasskeyCeremony {
        challenge,
        step_up: false,
        label: String::new(),
        created_at_unix: now_unix(),
    });
    if let Err(message) = save_economy(&economy, economy_path) {
        return err(500, message);
    }
    ok(serde_json::json!({
        "status": "ok",
        "challenge": webauthn::base64url_encode(&challenge),
        "rp": {
            "id": webauthn::RP_ID,
            "name": "AWENET Founder",
        },
        "user": {
            "id": identity.public.awe_id.to_hex(),
            "name": identity.public.awe_id.to_hex(),
            "display_name": "AWENET founder",
        },
        "pub_key_cred_params": [{ "type": "public-key", "alg": -8 }],
        "attestation": "none",
    }))
}

fn passkey_register(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    economy_path: &Path,
) -> ApiResponse {
    let mut economy = match lock_economy(economy_state) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let ceremony = match economy.passkey_ceremony.take() {
        Some(ceremony) if !ceremony.step_up => ceremony,
        Some(_) => {
            return err(
                400,
                "a step-up ceremony is pending; complete it first".to_string(),
            )
        }
        None => return err(400, "start a registration ceremony first".to_string()),
    };
    if now_unix() > ceremony.created_at_unix.saturating_add(CEREMONY_TTL_SECS) {
        return err(400, "the ceremony expired; start a new one".to_string());
    }
    let client_data_json = match parsed
        .get("client_data_json")
        .and_then(serde_json::Value::as_str)
    {
        Some(text) => text.to_string(),
        None => return err(400, "client_data_json is required".to_string()),
    };
    let ceremony_type = webauthn::client_data_field(&client_data_json, "type");
    if ceremony_type.as_deref() != Some("webauthn.create") {
        return err(400, "client data is not a creation ceremony".to_string());
    }
    match webauthn::client_data_field(&client_data_json, "challenge") {
        Some(challenge) if challenge == webauthn::base64url_encode(&ceremony.challenge) => {}
        _ => return err(400, "the ceremony challenge does not match".to_string()),
    }
    let attestation_object = match parsed
        .get("attestation_object")
        .and_then(serde_json::Value::as_str)
    {
        Some(text) => match webauthn::base64url_decode(text.trim()) {
            Ok(bytes) => bytes,
            Err(_) => return err(400, "attestation_object is not valid base64url".to_string()),
        },
        None => return err(400, "attestation_object is required".to_string()),
    };
    let (authenticator_data, _format) =
        match webauthn::parse_attestation_object(&attestation_object) {
            Ok(attestation) => attestation,
            Err(message) => {
                return err(400, format!("the attestation is not acceptable: {message}"))
            }
        };
    if authenticator_data.rp_id_hash != webauthn::rp_id_hash() {
        return err(
            400,
            "the credential is bound to a different relying party".to_string(),
        );
    }
    let attested = match authenticator_data.attested_credential {
        Some(attested) => attested,
        None => return err(400, "the attestation carries no credential".to_string()),
    };
    ensure_founder_guard(&mut economy, identity);
    // The founder node key signs the enrollment authorization locally on
    // the founder machine; the signature never travels the wire.
    let founder_signature = identity.sign(&FounderGuard::enrollment_message(&attested.public_key));
    let label = parsed
        .get("label")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("founder passkey");
    let now = now_unix();
    if let Err(message) = economy
        .founder_guard
        .as_mut()
        .expect("the founder guard was just ensured")
        .enroll_passkey_bytes(&founder_signature, &attested.public_key, label, now)
    {
        return err(400, message.to_string());
    }
    if let Err(message) = save_economy(&economy, economy_path) {
        return err(500, message);
    }
    ok(serde_json::json!({
        "status": "enrolled",
        "credential_id_hex": hex::encode(attested.credential_id),
        "label": label,
    }))
}

fn passkey_step_up_options(
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
    ensure_founder_guard(&mut economy, identity);
    if economy
        .founder_guard
        .as_ref()
        .expect("the founder guard was just ensured")
        .passkeys()
        .is_empty()
    {
        return err(
            400,
            "enroll a passkey before requesting a step-up".to_string(),
        );
    }
    let challenge: [u8; 32] = *secure_random::<32>();
    economy
        .founder_guard
        .as_mut()
        .expect("the founder guard was just ensured")
        .begin_step_up();
    economy.passkey_ceremony = Some(PasskeyCeremony {
        challenge,
        step_up: true,
        label: String::new(),
        created_at_unix: now_unix(),
    });
    if let Err(message) = save_economy(&economy, economy_path) {
        return err(500, message);
    }
    ok(serde_json::json!({
        "status": "ok",
        "challenge": webauthn::base64url_encode(&challenge),
        "rp_id": webauthn::RP_ID,
        "user_verification": "preferred",
    }))
}

fn passkey_step_up(
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
    let ceremony = match economy.passkey_ceremony.take() {
        Some(ceremony) if ceremony.step_up => ceremony,
        Some(_) => {
            return err(
                400,
                "a registration ceremony is pending; complete it first".to_string(),
            )
        }
        None => return err(400, "start a step-up ceremony first".to_string()),
    };
    if now_unix() > ceremony.created_at_unix.saturating_add(CEREMONY_TTL_SECS) {
        return err(400, "the ceremony expired; start a new one".to_string());
    }
    let client_data_json = match parsed
        .get("client_data_json")
        .and_then(serde_json::Value::as_str)
    {
        Some(text) => text.to_string(),
        None => return err(400, "client_data_json is required".to_string()),
    };
    let ceremony_type = webauthn::client_data_field(&client_data_json, "type");
    if ceremony_type.as_deref() != Some("webauthn.get") {
        return err(400, "client data is not an assertion".to_string());
    }
    match webauthn::client_data_field(&client_data_json, "challenge") {
        Some(challenge) if challenge == webauthn::base64url_encode(&ceremony.challenge) => {}
        _ => return err(400, "the step-up challenge does not match".to_string()),
    }
    let authenticator_data = match parsed
        .get("authenticator_data")
        .and_then(serde_json::Value::as_str)
    {
        Some(text) => match webauthn::base64url_decode(text.trim()) {
            Ok(bytes) => bytes,
            Err(_) => return err(400, "authenticator_data is not valid base64url".to_string()),
        },
        None => return err(400, "authenticator_data is required".to_string()),
    };
    let signature = match parsed.get("signature").and_then(serde_json::Value::as_str) {
        Some(text) => match webauthn::base64url_decode(text.trim()) {
            Ok(bytes) if bytes.len() == 64 => {
                let mut signature = [0u8; 64];
                signature.copy_from_slice(&bytes);
                signature
            }
            _ => return err(400, "signature must be 64 bytes of base64url".to_string()),
        },
        None => return err(400, "signature is required".to_string()),
    };
    ensure_founder_guard(&mut economy, identity);
    let enrolled = economy
        .founder_guard
        .as_ref()
        .expect("the founder guard was just ensured")
        .passkeys()
        .to_vec();
    let possession_message =
        webauthn::assertion_message(&authenticator_data, client_data_json.as_bytes());
    let credential_public_key = enrolled
        .iter()
        .find(|passkey| {
            webauthn::verify_assertion(
                &passkey.public_key,
                &authenticator_data,
                client_data_json.as_bytes(),
                &signature,
            )
        })
        .map(|passkey| passkey.public_key);
    let Some(credential_public_key) = credential_public_key else {
        return err(
            403,
            "no enrolled passkey produced a valid assertion".to_string(),
        );
    };
    if let Err(message) = economy
        .founder_guard
        .as_mut()
        .expect("the founder guard was just ensured")
        .complete_step_up_assertion(
            &credential_public_key,
            &possession_message,
            &signature,
            now_unix(),
        )
    {
        return err(403, message.to_string());
    }
    let expires_at_unix = economy
        .founder_guard
        .as_ref()
        .expect("the founder guard was just ensured")
        .session()
        .map(|session| session.expires_at_unix);
    if let Err(message) = save_economy(&economy, economy_path) {
        return err(500, message);
    }
    ok(serde_json::json!({
        "status": "ok",
        "session_active": true,
        "expires_at_unix": expires_at_unix,
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
    if let Some(response) = require_founder_action(&economy, identity) {
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
    let NodeEconomy {
        price_control,
        authority,
        ..
    } = &mut *economy;
    match price_control.set_price(
        authority,
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
    if let Some(response) = require_founder_action(&economy, identity) {
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
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    match market.approve_and_pay(authority, identity, &mut ledger, submission_id, now) {
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
    if let Some(response) = require_founder_action(&economy, identity) {
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
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    match market.approve_and_pay_team(authority, identity, &mut ledger, submission_id, &split, now)
    {
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
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let reason = match parsed.get("reason").and_then(serde_json::Value::as_str) {
        Some(reason) => reason.trim().to_string(),
        None => return err(400, "a reason is required".to_string()),
    };
    let now = now_unix();
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    let result = match kind {
        ReviewKind::Reject => market.reject_submission(
            authority,
            &identity.public.public_key,
            submission_id,
            &reason,
            now,
        ),
        ReviewKind::Revise => market.request_revision(
            authority,
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
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let now = now_unix();
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    let result = match kind {
        TaskClose::Close => market.close_task(authority, &identity.public.public_key, task_id, now),
        TaskClose::Cancel => {
            market.cancel_task(authority, &identity.public.public_key, task_id, now)
        }
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
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let NodeEconomy {
        market, authority, ..
    } = &mut *economy;
    match market.approve_suggestion(
        authority,
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
    if let Some(response) = require_founder_action(&economy, identity) {
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
    let NodeEconomy {
        store_escrow,
        authority,
        ..
    } = &mut *economy;
    match store_escrow.resolve_dispute(
        authority,
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
    if let Some(response) = require_founder_action(&economy, identity) {
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

/// The network-wide storefront catalog: the products other nodes
/// announced over the marketplace sync stream. This is discovery data,
/// honestly bounded: a listing shows where the seller node accepts
/// buyers, it is not an order route, and a purchase still settles on
/// the seller's own ledger. Cross-node order execution is not
/// implemented.
fn network_catalog(
    params: &BTreeMap<String, String>,
    marketplace_state: &RemoteCatalogState,
) -> ApiResponse {
    let catalog = match marketplace_state.lock() {
        Ok(guard) => guard,
        Err(_) => return err(500, "marketplace catalog is locked".to_string()),
    };
    let kind = match params
        .get("kind")
        .map(|text| ProductKind::parse(text).ok_or_else(|| format!("unknown product kind: {text}")))
    {
        Some(Ok(kind)) => Some(kind),
        Some(Err(message)) => return err(400, message),
        None => None,
    };
    let seller = match params.get("seller") {
        Some(text) => match AweId::from_hex(text) {
            Ok(seller) => Some(seller),
            Err(message) => return err(400, message),
        },
        None => None,
    };
    let filter = ProductFilter {
        query: params.get("q").cloned().unwrap_or_default(),
        kind,
        category: params.get("category").cloned(),
        seller,
        min_price_atoms: params
            .get("min_price_atoms")
            .and_then(|text| text.parse::<u128>().ok())
            .unwrap_or(0),
        max_price_atoms: params
            .get("max_price_atoms")
            .and_then(|text| text.parse::<u128>().ok()),
        include_inactive: false,
    };
    let listings: Vec<serde_json::Value> = catalog
        .search(&filter)
        .iter()
        .map(|listing| {
            serde_json::json!({
                "id": listing.core.product_id,
                "seller": listing.core.seller.to_hex(),
                "seller_node_address": listing.core.seller_node_address,
                "title": listing.core.title,
                "description": listing.core.description,
                "kind": listing.core.kind.as_str(),
                "category": listing.core.category,
                "price": money_json(listing.core.price_atoms),
                "stock": listing.core.stock,
                "listed_at_unix": listing.first_seen_unix,
                "last_seen_unix": listing.last_seen_unix,
            })
        })
        .collect();
    ok(serde_json::json!({
        "status": "ok",
        "listings": listings,
        "count": listings.len(),
        "notice": "listings are announced by their seller nodes over the marketplace sync stream; a listing points at the seller's node and is not an order route: purchases settle on the seller's own ledger and cross-node order execution is not implemented"
    }))
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
                let amount_atoms = economy
                    .store_escrow
                    .order(order_id)
                    .map(|order| order.amount_atoms)
                    .unwrap_or(0);
                if amount_atoms > 0 {
                    let _ = economy
                        .treasury
                        .record(TreasuryFlow::EscrowRefund, amount_atoms, now);
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

// ---------------------------------------------------------------------------
// Multi-node usage verification: the verifier registry, attestation
// windows and report intake of the live verifier transport.
// ---------------------------------------------------------------------------

/// Persists the verifier net as a founder-signed snapshot so a restart
/// never loses the enrolled verifier roster, open attestation windows or
/// the reports collected for them.
fn save_verifier_net(
    verifier_net: &VerifierNet,
    verifier_path: &Path,
    identity: &Identity,
) -> Result<(), String> {
    verifier_net_store::save(verifier_path, verifier_net, identity)
        .map_err(|error| error.to_string())
}

/// `GET /api/verifier/net`: the live verification state of this node.
/// Read-only: the founder-signed verifier roster, every attestation
/// window with the reports collected for it and the consensus reached
/// so far. Nothing here mints ONECOIN; settlement stays with the
/// resource economy and reads the consensus this endpoint exposes.
fn verifier_net_status(verifier_state: &VerifierNetState) -> ApiResponse {
    let Ok(verifier_net) = verifier_state.lock() else {
        return err(500, "verifier state lock failed".to_string());
    };
    let now = now_unix();
    let verifiers: Vec<serde_json::Value> = verifier_net
        .registry()
        .verifiers()
        .map(|certificate| {
            serde_json::json!({
                "verifier": certificate.verifier.to_hex(),
                "public_key": hex::encode(certificate.public_key),
                "enrolled_at_unix": certificate.enrolled_at_unix,
                "retired_at_unix": certificate.retired_at_unix,
                "active": certificate.is_active(),
            })
        })
        .collect();
    let mut windows = Vec::new();
    for window in verifier_net.windows() {
        let reports = verifier_net.reports_for(&window.node, window.period_start_unix);
        windows.push(serde_json::json!({
            "node": window.node.to_hex(),
            "period_start_unix": window.period_start_unix,
            "period_end_unix": window.period_end_unix,
            "report_deadline_unix": window.report_deadline_unix,
            "closed": window.closed,
            "reports": reports.len(),
            "has_consensus": verifier_net.has_consensus(
                &window.node,
                window.period_start_unix,
                window.period_end_unix,
            ),
        }));
    }
    ok(serde_json::json!({
        "active_verifiers": verifier_net.registry().active_verifiers(),
        "verifiers": verifiers,
        "windows": windows,
        "now_unix": now,
    }))
}

fn parse_hex_32(value: &serde_json::Value, field: &str) -> Result<[u8; 32], ApiResponse> {
    let text = value
        .get(field)
        .and_then(|field| field.as_str())
        .ok_or_else(|| err(400, format!("{field} is required (hex)")))?;
    let bytes = hex::decode(text).map_err(|_| err(400, format!("{field} must be valid hex")))?;
    let key: [u8; 32] = bytes.try_into().map_err(|_| {
        err(
            400,
            format!("{field} must be exactly 32 bytes (64 hex chars)"),
        )
    })?;
    Ok(key)
}

fn parse_hex_64(value: &serde_json::Value, field: &str) -> Result<[u8; 64], ApiResponse> {
    let text = value
        .get(field)
        .and_then(|field| field.as_str())
        .ok_or_else(|| err(400, format!("{field} is required (hex)")))?;
    let bytes = hex::decode(text).map_err(|_| err(400, format!("{field} must be valid hex")))?;
    let signature: [u8; 64] = bytes.try_into().map_err(|_| {
        err(
            400,
            format!("{field} must be exactly 64 bytes (128 hex chars)"),
        )
    })?;
    Ok(signature)
}

/// Parses one signed usage report from the wire format the live
/// transport also carries: every field is hex or plain, the signature
/// must verify before the report is ever offered to the verifier net.
fn parse_signed_report(value: &serde_json::Value) -> Result<SignedUsageReport, ApiResponse> {
    let node_text = value
        .get("node")
        .and_then(|node| node.as_str())
        .ok_or_else(|| err(400, "node is required (hex AWE id)".to_string()))?;
    let node = AweId::from_hex(node_text)
        .map_err(|message| err(400, format!("node is not a valid AWE id: {message}")))?;
    let period_start_unix = value
        .get("period_start_unix")
        .and_then(|field| field.as_u64())
        .ok_or_else(|| err(400, "period_start_unix is required".to_string()))?;
    let period_end_unix = value
        .get("period_end_unix")
        .and_then(|field| field.as_u64())
        .ok_or_else(|| err(400, "period_end_unix is required".to_string()))?;
    let usage = value
        .get("usage")
        .cloned()
        .and_then(|usage| serde_json::from_value::<MeasuredUsage>(usage).ok())
        .ok_or_else(|| err(400, "usage must be a measured usage object".to_string()))?;
    let verifier_public_key = parse_hex_32(value, "verifier_public_key")?;
    let signature = parse_hex_64(value, "signature")?;
    let report = SignedUsageReport {
        node,
        period_start_unix,
        period_end_unix,
        usage,
        verifier_public_key,
        signature,
    };
    if !report.verify() {
        return Err(err(
            400,
            "the usage report signature is invalid".to_string(),
        ));
    }
    Ok(report)
}

/// `POST /api/verifier/reports`: intake for one signed verifier report,
/// the HTTP twin of the live transport. The verifier net enforces every
/// acceptance rule (enrolled active verifier, open window, no
/// self-attestation, no duplicates); a rejected report never reaches a
/// settlement. Accepted reports persist immediately in the
/// founder-signed snapshot.
fn verifier_submit_report(
    parsed: &serde_json::Value,
    identity: &Identity,
    verifier_state: &VerifierNetState,
    verifier_path: &Path,
) -> ApiResponse {
    let report = match parse_signed_report(parsed) {
        Ok(report) => report,
        Err(response) => return response,
    };
    let now = now_unix();
    let Ok(mut verifier_net) = verifier_state.lock() else {
        return err(500, "verifier state lock failed".to_string());
    };
    match verifier_net.submit_report(report, now) {
        Ok(()) => {}
        Err(message) => return err(400, message.to_string()),
    }
    if let Err(message) = save_verifier_net(&verifier_net, verifier_path, identity) {
        return err(
            500,
            format!("verifier snapshot persistence failed: {message}"),
        );
    }
    ok(serde_json::json!({ "status": "accepted" }))
}

/// `POST /api/founder/verifier/enroll`: the founder enrolls a live peer
/// node as a usage verifier by its public node key. Founder-only and
/// backend-gated exactly like every other founder action; the economy
/// lock is always taken before the verifier lock.
fn founder_enroll_verifier(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    verifier_state: &VerifierNetState,
    verifier_path: &Path,
) -> ApiResponse {
    let Ok(economy) = economy_state.lock() else {
        return err(500, "economy state lock failed".to_string());
    };
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let verifier_public_key = match parse_hex_32(parsed, "public_key") {
        Ok(key) => key,
        Err(response) => return response,
    };
    // the AWE id of a live peer node is the deterministic hash of its
    // public key, so the founder can enroll a running peer by its
    // announced public key alone
    let node = AweId::from_public_key(&verifier_public_key);
    let now = now_unix();
    let Ok(mut verifier_net) = verifier_state.lock() else {
        return err(500, "verifier state lock failed".to_string());
    };
    let certificate =
        match verifier_net.enroll_verifier_public(identity, &node, &verifier_public_key, now) {
            Ok(certificate) => certificate,
            Err(message) => return err(400, message.to_string()),
        };
    if let Err(message) = save_verifier_net(&verifier_net, verifier_path, identity) {
        return err(
            500,
            format!("verifier snapshot persistence failed: {message}"),
        );
    }
    ok(serde_json::json!({
        "status": "enrolled",
        "verifier": certificate.verifier.to_hex(),
        "public_key": hex::encode(certificate.public_key),
        "enrolled_at_unix": certificate.enrolled_at_unix,
    }))
}

/// `POST /api/founder/verifier/retire`: the founder retires an enrolled
/// verifier by its public key; retired verifiers stop being accepted for
/// new reports.
fn founder_retire_verifier(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    verifier_state: &VerifierNetState,
    verifier_path: &Path,
) -> ApiResponse {
    let Ok(economy) = economy_state.lock() else {
        return err(500, "economy state lock failed".to_string());
    };
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let verifier_public_key = match parse_hex_32(parsed, "public_key") {
        Ok(key) => key,
        Err(response) => return response,
    };
    let now = now_unix();
    let Ok(mut verifier_net) = verifier_state.lock() else {
        return err(500, "verifier state lock failed".to_string());
    };
    let certificate = match verifier_net.retire_verifier(identity, &verifier_public_key, now) {
        Ok(certificate) => certificate,
        Err(message) => return err(400, message.to_string()),
    };
    if let Err(message) = save_verifier_net(&verifier_net, verifier_path, identity) {
        return err(
            500,
            format!("verifier snapshot persistence failed: {message}"),
        );
    }
    ok(serde_json::json!({
        "status": "retired",
        "verifier": certificate.verifier.to_hex(),
        "public_key": hex::encode(certificate.public_key),
    }))
}

/// `POST /api/founder/verifier/window`: the founder opens one
/// attestation window for a contributing node (default: this node).
/// The transport announces the window to the peers, which then deliver
/// their signed reports through the verifier transport stream or this
/// HTTP intake.
fn founder_open_window(
    parsed: &serde_json::Value,
    identity: &Identity,
    economy_state: &EconomyState,
    verifier_state: &VerifierNetState,
    verifier_path: &Path,
) -> ApiResponse {
    let Ok(economy) = economy_state.lock() else {
        return err(500, "economy state lock failed".to_string());
    };
    if let Some(response) = require_founder_action(&economy, identity) {
        return response;
    }
    let node = match parsed.get("node").and_then(|node| node.as_str()) {
        Some(hex) => match AweId::from_hex(hex) {
            Ok(node) => node,
            Err(message) => return err(400, format!("node is not a valid AWE id: {message}")),
        },
        None => identity.public.awe_id.clone(),
    };
    let period_secs = match parsed.get("period_secs").and_then(|field| field.as_u64()) {
        Some(secs) => secs,
        None => return err(400, "period_secs is required".to_string()),
    };
    let period_start_unix = parsed
        .get("period_start_unix")
        .and_then(|field| field.as_u64())
        .unwrap_or_else(now_unix);
    let period_end_unix = match period_start_unix.checked_add(period_secs) {
        Some(end) => end,
        None => return err(400, "the settlement period overflows".to_string()),
    };
    let now = now_unix();
    let Ok(mut verifier_net) = verifier_state.lock() else {
        return err(500, "verifier state lock failed".to_string());
    };
    let window = match verifier_net.open_window(&node, period_start_unix, period_end_unix, now) {
        Ok(window) => window,
        Err(message) => return err(400, message.to_string()),
    };
    if let Err(message) = save_verifier_net(&verifier_net, verifier_path, identity) {
        return err(
            500,
            format!("verifier snapshot persistence failed: {message}"),
        );
    }
    ok(serde_json::json!({
        "status": "open",
        "node": window.node.to_hex(),
        "period_start_unix": window.period_start_unix,
        "period_end_unix": window.period_end_unix,
        "report_deadline_unix": window.report_deadline_unix,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "awenet-verifier-api-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp verifier api dir");
        path
    }

    fn report_body(report: &SignedUsageReport) -> String {
        serde_json::json!({
            "node": report.node.to_hex(),
            "period_start_unix": report.period_start_unix,
            "period_end_unix": report.period_end_unix,
            "usage": report.usage.clone(),
            "verifier_public_key": hex::encode(report.verifier_public_key),
            "signature": hex::encode(report.signature),
        })
        .to_string()
    }

    #[test]
    fn verifier_endpoints_reject_everyone_but_the_founder() {
        let founder = identity("api-founder");
        let stranger = identity("api-stranger");
        let verifier = identity("api-verifier");
        let dir = temp_dir("gate");
        let (economy, _created) = load_or_create(&founder, &dir.join("economy.json"));
        let economy_state: EconomyState = Arc::new(Mutex::new(economy));
        let ledger: LedgerState = Arc::new(Mutex::new(OnecoinLedger::default()));
        let verifier_state: VerifierNetState = Arc::new(Mutex::new(VerifierNet::new(&founder, 0)));
        let body = serde_json::json!({
            "public_key": hex::encode(verifier.public.public_key)
        })
        .to_string();
        let (status, _mime, response) = handle(
            "POST",
            "/api/founder/verifier/enroll",
            &body,
            &stranger,
            &ledger,
            &economy_state,
            &dir.join("economy.json"),
            &dir.join("ledger.json"),
            &verifier_state,
            &dir.join("verifier-net.json"),
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "403 Forbidden");
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let message = parsed
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("");
        assert!(
            message.contains("only the AWENET founder"),
            "a stranger must not enroll verifiers: {response}"
        );
        // the founder-only window endpoint is gated the same way
        let (status, _mime, _response) = handle(
            "POST",
            "/api/founder/verifier/window",
            r#"{"period_secs":3600}"#,
            &stranger,
            &ledger,
            &economy_state,
            &dir.join("economy.json"),
            &dir.join("ledger.json"),
            &verifier_state,
            &dir.join("verifier-net.json"),
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "403 Forbidden");
    }

    #[test]
    fn verifier_flow_enrolls_opens_reports_and_persists() {
        let founder = identity("flow-founder");
        let verifier_a = identity("flow-verifier-a");
        let verifier_b = identity("flow-verifier-b");
        let dir = temp_dir("flow");
        let economy_path = dir.join("economy.json");
        let ledger_path = dir.join("ledger.json");
        let verifier_path = dir.join("verifier-net.json");
        let (economy, _created) = load_or_create(&founder, &economy_path);
        let economy_state: EconomyState = Arc::new(Mutex::new(economy));
        let ledger: LedgerState = Arc::new(Mutex::new(OnecoinLedger::default()));
        let verifier_state: VerifierNetState = Arc::new(Mutex::new(VerifierNet::new(&founder, 0)));

        // the founder enrolls two verifiers by their public node keys
        for verifier in [&verifier_a, &verifier_b] {
            let body = serde_json::json!({
                "public_key": hex::encode(verifier.public.public_key)
            })
            .to_string();
            let (status, _mime, response) = handle(
                "POST",
                "/api/founder/verifier/enroll",
                &body,
                &founder,
                &ledger,
                &economy_state,
                &economy_path,
                &ledger_path,
                &verifier_state,
                &verifier_path,
                &Arc::new(Mutex::new(RemoteCatalog::new())),
            );
            assert_eq!(status, "200 OK");
            assert!(response.contains(r#""status":"enrolled""#), "{response}");
        }
        // a verifier cannot be enrolled twice
        let body = serde_json::json!({
            "public_key": hex::encode(verifier_a.public.public_key)
        })
        .to_string();
        let (status, _mime, response) = handle(
            "POST",
            "/api/founder/verifier/enroll",
            &body,
            &founder,
            &ledger,
            &economy_state,
            &economy_path,
            &ledger_path,
            &verifier_state,
            &verifier_path,
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "400 Bad Request");
        assert!(response.contains("already enrolled"), "{response}");

        // the founder opens a one hour attestation window for this node
        let (status, _mime, response) = handle(
            "POST",
            "/api/founder/verifier/window",
            r#"{"period_secs":3600}"#,
            &founder,
            &ledger,
            &economy_state,
            &economy_path,
            &ledger_path,
            &verifier_state,
            &verifier_path,
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "200 OK");
        let opened: serde_json::Value = serde_json::from_str(&response).unwrap();
        let period_start = opened
            .get("period_start_unix")
            .and_then(|value| value.as_u64())
            .unwrap();
        let period_end = opened
            .get("period_end_unix")
            .and_then(|value| value.as_u64())
            .unwrap();
        assert_eq!(period_end, period_start + 3_600);

        // both verifiers deliver one signed report each through the
        // HTTP intake, the twin of the live transport
        let node = founder.public.awe_id.clone();
        let usage = MeasuredUsage {
            bandwidth_gb: 3,
            uptime_hours: 1,
            ..MeasuredUsage::default()
        };
        for verifier in [&verifier_a, &verifier_b] {
            let report = SignedUsageReport::new(
                verifier,
                node.clone(),
                period_start,
                period_end,
                usage.clone(),
            );
            let (status, _mime, response) = handle(
                "POST",
                "/api/verifier/reports",
                &report_body(&report),
                &founder,
                &ledger,
                &economy_state,
                &economy_path,
                &ledger_path,
                &verifier_state,
                &verifier_path,
                &Arc::new(Mutex::new(RemoteCatalog::new())),
            );
            assert_eq!(status, "200 OK", "{response}");
        }
        // a replay of the same report is a duplicate and never counts twice
        let replay = SignedUsageReport::new(
            &verifier_a,
            node.clone(),
            period_start,
            period_end,
            usage.clone(),
        );
        let (status, _mime, response) = handle(
            "POST",
            "/api/verifier/reports",
            &report_body(&replay),
            &founder,
            &ledger,
            &economy_state,
            &economy_path,
            &ledger_path,
            &verifier_state,
            &verifier_path,
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "400 Bad Request");
        assert!(response.contains("already reported"), "{response}");
        // a tampered signature never reaches the verifier net
        let mut forged = report_body(&replay);
        forged.pop();
        forged.pop();
        forged.push_str("00\"}");
        let (status, _mime, _response) = handle(
            "POST",
            "/api/verifier/reports",
            &forged,
            &founder,
            &ledger,
            &economy_state,
            &economy_path,
            &ledger_path,
            &verifier_state,
            &verifier_path,
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "400 Bad Request");

        // the public status exposes the consensus the resource economy
        // settles from
        let (status, _mime, response) = handle(
            "GET",
            "/api/verifier/net",
            "",
            &founder,
            &ledger,
            &economy_state,
            &economy_path,
            &ledger_path,
            &verifier_state,
            &verifier_path,
            &Arc::new(Mutex::new(RemoteCatalog::new())),
        );
        assert_eq!(status, "200 OK");
        let net: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(
            net.get("active_verifiers").and_then(|v| v.as_u64()),
            Some(2),
            "{net}"
        );
        let windows = net.get("windows").and_then(|v| v.as_array()).unwrap();
        let window = windows
            .iter()
            .find(|window| {
                window.get("period_start_unix").and_then(|v| v.as_u64()) == Some(period_start)
            })
            .unwrap();
        assert_eq!(window.get("reports").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(
            window.get("has_consensus").and_then(|v| v.as_bool()),
            Some(true)
        );

        // the verifier net persists as a founder-signed snapshot that
        // survives a restart
        let reloaded = verifier_net_store::load(&verifier_path, &founder).unwrap();
        assert_eq!(reloaded.registry().active_verifiers(), 2);
        assert!(reloaded.has_consensus(&node, period_start, period_end));
    }

    #[test]
    fn network_catalog_exposes_peer_listings_as_discovery_data() {
        use awep2p_core::marketplace_sync::{
            ListingAnnounce, ListingAnnounceCore, MARKETPLACE_SYNC_VERSION,
        };

        let founder = identity("api-marketplace-founder");
        let seller = identity("api-marketplace-seller");
        let dir = temp_dir("marketplace");
        let (economy, _created) = load_or_create(&founder, &dir.join("economy.json"));
        let economy_state: EconomyState = Arc::new(Mutex::new(economy));
        let ledger: LedgerState = Arc::new(Mutex::new(OnecoinLedger::default()));
        let verifier_state: VerifierNetState = Arc::new(Mutex::new(VerifierNet::new(&founder, 0)));
        let marketplace_state: RemoteCatalogState = Arc::new(Mutex::new(RemoteCatalog::new()));
        // the seller node announces one of its own products over the
        // marketplace sync stream
        let announce = ListingAnnounce::sign(
            ListingAnnounceCore {
                version: MARKETPLACE_SYNC_VERSION,
                product_id: 7,
                seller: seller.public.awe_id.clone(),
                seller_public_key: seller.public.public_key,
                seller_node_address: "127.0.0.1:46202".into(),
                title: "AWE Theme Pack".into(),
                description: "Dark and light themes".into(),
                kind: ProductKind::Digital,
                category: "themes".into(),
                price_atoms: 250,
                stock: None,
                listed: true,
                revision_unix: 1_000,
            },
            &seller,
        )
        .unwrap();
        marketplace_state
            .lock()
            .unwrap()
            .merge(seller.public.awe_id.as_bytes(), &announce, 1_000)
            .unwrap();
        let (status, _mime, response) = handle(
            "GET",
            "/api/storefront/network",
            "",
            &founder,
            &ledger,
            &economy_state,
            &dir.join("economy.json"),
            &dir.join("ledger.json"),
            &verifier_state,
            &dir.join("verifier-net.json"),
            &marketplace_state,
        );
        assert_eq!(status, "200 OK", "{response}");
        let catalog: serde_json::Value = serde_json::from_str(&response).unwrap();
        let listings = catalog.get("listings").and_then(|v| v.as_array()).unwrap();
        assert_eq!(listings.len(), 1, "{catalog}");
        let listing = &listings[0];
        assert_eq!(listing.get("id").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(
            listing.get("seller").and_then(|v| v.as_str()),
            Some(seller.public.awe_id.to_hex().as_str())
        );
        assert_eq!(
            listing.get("seller_node_address").and_then(|v| v.as_str()),
            Some("127.0.0.1:46202")
        );
        // the honest notice keeps the listing from being mistaken for
        // an order route
        let notice = catalog.get("notice").and_then(|v| v.as_str()).unwrap();
        assert!(notice.contains("not an order route"), "{notice}");
        // the established filter surface applies to network listings
        let (status, _mime, response) = handle(
            "GET",
            "/api/storefront/network?q=missing",
            "",
            &founder,
            &ledger,
            &economy_state,
            &dir.join("economy.json"),
            &dir.join("ledger.json"),
            &verifier_state,
            &dir.join("verifier-net.json"),
            &marketplace_state,
        );
        assert_eq!(status, "200 OK", "{response}");
        let filtered: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(filtered.get("count").and_then(|v| v.as_u64()), Some(0));
        // an unknown seller filter is a 400, not a silent empty list
        let (status, _mime, response) = handle(
            "GET",
            "/api/storefront/network?seller=not-hex",
            "",
            &founder,
            &ledger,
            &economy_state,
            &dir.join("economy.json"),
            &dir.join("ledger.json"),
            &verifier_state,
            &dir.join("verifier-net.json"),
            &marketplace_state,
        );
        assert_eq!(status, "400 Bad Request", "{response}");
    }
}
