# AWENET Phase 1 — Component Map and Verification Checklist

**Baseline:** `main@e97809f3eb7e0de09b4fa7c2979ac4bcedb0936b`  
**Companion audit:** [PHASE-1-REPOSITORY-AUDIT.md](PHASE-1-REPOSITORY-AUDIT.md)

This map separates the existing code surface from the behavior that still needs proof. It is intended to prevent Phase 2 from adding duplicate modules or mistaking a data structure for an operational service.

## Component-by-component map

| Component | Existing source surface | What must be proven before marking complete |
|---|---|---|
| Workspace/build | Root `Cargo.toml`, `Cargo.lock`, CI workflows | Reproducible build from clean checkout; dependency audit; supported toolchain/platform matrix |
| AWEID / local identity | `core/src/identity.rs` | Stable ID derivation; vault encryption and password errors; no secret leakage; safe recovery/rotation policy |
| Access / permissions | `core/src/access.rs`, `permissions.rs`, `policy.rs` | Backend-enforced authorization on every protected operation; default-deny behavior; role revocation |
| Secure transport | `core/src/network.rs`, `protocol.rs`, `crypto.rs` | Peer authentication, session key derivation, AEAD nonce uniqueness, replay rejection, frame limits, timeout behavior |
| Peer lifecycle | `network.rs`, `node.rs`, `supervisor.rs` | Real socket connection, observed peer state, reconnect/backoff, shutdown, duplicate connection resolution, persistence |
| Discovery/routing | `discovery.rs`, `routing.rs`, `network_topology.rs`, `lan_mesh.rs` | Multiple discovery sources, bounded lookup/hops, stale record rejection, multi-hop request/response evidence |
| Federation | `federation.rs`, node API federation routes | Authenticated config import/export, signature/freshness validation, replay-safe membership updates |
| Resource IDs/manifests | `storage.rs`, `canonical.rs`, `namespace.rs`, `registry.rs` | Canonical IDs; immutable resource identity; verified ownership/sequence/freshness; mutable names without ID changes |
| Local storage | `storage.rs` | Quota enforcement, content integrity, safe atomic writes, GC safety, encrypted private payload handling, corruption recovery |
| Distributed data plane | `data_plane.rs`, `host.rs`, `host_directory.rs`, `network.rs` | Actual remote byte transfer; per-chunk integrity; ACK only after durable verified write; resume/timeout semantics |
| Placement/replication | `replication.rs`, `product.rs`, `storage.rs` | Distinct healthy nodes counted correctly; placement metadata follows actual writes; failed peers are not counted as replicas |
| Repair/recovery | `repair.rs`, `recovery.rs`, node repair cycle | Real bytes moved/reconstructed, independent health evidence, no unsafe deletion, idempotent metadata commits |
| Limits/defense | `limits.rs`, `defense.rs`, `replay.rs`, `reputation.rs` | Every externally influenced collection bounded; abuse limits not bypassable; replay protection across restart where required |
| Diagnostics/readiness | `diagnostics.rs`, `readiness.rs` | Status reflects observed runtime state; no fake “healthy” defaults; secrets/private payloads excluded from logs |
| Browser / sites | `namespace.rs`, `registry.rs`, node storage/site API, desktop JS | Global search/discovery; signed site records; remote fetch by stable ID; integrity and authorization; safe HTML/content handling |
| Messenger | `messenger.rs`, `messenger_runtime.rs`, API handlers | Sender identity verified; durable/offline queue; remote ACK semantics; retries and duplicate suppression |
| Calls | `calls.rs`, call signaling API, UI | Signaling is authenticated; media transport is truly encrypted and end-to-end; UI does not imply a call is connected prematurely |
| Groups/channels | Messenger/community logic and API handlers | Owner/admin checks, subscriber identity verification, persistence, remote delivery, unauthorized publication rejection |
| Store / app packages | `onecoin_store.rs`, `sandbox.rs`, `registry.rs`, Store API | Package hash/signature checks, publisher identity, permission grants, runtime sandbox enforcement, rollback/uninstall |
| ONECOIN ledger | `onecoin.rs`, `onecoin_wallet.rs` | Atomic integer arithmetic; valid signatures; sender nonce/replay checks; no overdraft; persistence and recovery |
| ONECOIN consensus | `onecoin_consensus.rs`, `onecoin_consensus_runtime.rs` | Validator set agreed across peers; quorum certificate fully verified; duplicate/equivocating votes handled; deterministic state transition |
| ONECOIN governance/payments | `onecoin_governance.rs`, `onecoin_payment.rs`, `onecoin_store.rs` | Proposal eligibility/quorum; invoice binding; amount/currency/expiry matching; no unilateral floor reduction |
| ONEBANK | `onebank.rs`, wallet/contribution/exchange API | Contribution receipt verification; reward idempotency; bounded fee; sender debit + receiver credit consistency; no implication of fiat custody |
| Admin / cloud control plane | Tier primitives in `onebank.rs`; node/API surfaces | Not yet demonstrated by a named complete control-plane service: define roles, tenants, resource inventory, quotas, lifecycle, audit log, isolation, and recovery |
| Node/server/DC/group operations | Tier/benefit primitives and node runtime | Real provisioning and deprovisioning, capacity accounting, access delegation, health monitoring, limits, and billing/reward evidence |
| Desktop client | `apps/awe-desktop/ui/*`, `apps/awe-app/src-tauri/*`, `native_ui.rs` | User-requested non-Tauri native UI; no separate browser window; stable backend connection; keyboard/accessibility and clean startup |
| Platform integration | `platforms/*`, launch scripts, cross-platform workflow | Correct lifecycle per OS, service permissions, clean install/uninstall, reproducible packages and checksums |
| Process smoke tests | `apps/awe-node/tests/product_smoke.rs` | Independent processes and distinct storage roots; assert remote bytes and durable ACK; exercise failure/reconnect/restart; avoid mocks hiding transport bugs |

## API contract matrix to create in Phase 2

For each route in `apps/awe-node/src/main.rs`, record:

1. HTTP method and exact path.
2. Request schema, maximum size, and validation.
3. Required identity/session/role and ownership check.
4. State touched: in-memory, local persistent, remote peer, or consensus-finalized.
5. Side effects and idempotency key/nonce.
6. Success response and the exact evidence that permits that response.
7. Error status/body for invalid input, unauthorized, missing resource, timeout, unavailable peer, and persistence failure.
8. Unit test, process test, multi-node test, and CI run covering the behavior.

The current visible API families are status/health, connect/federation, storage, Messenger, groups/channels, call signaling, Store, security, and ONEBANK wallet/contribution/transfers/exchange offers. Do not expose an admin route until its authorization model and persistence behavior are implemented and tested.

## Required end-to-end traces

### A. Identity to network membership
Generate identity -> seal vault -> reopen vault -> start node -> authenticate a distinct peer -> reject wrong key/password -> reconnect after peer restart -> confirm no identity or key leakage in logs.

### B. Storage to verified remote durability
Create content ID/manifest -> encrypt private content -> select distinct peers -> transfer chunks -> verify remote integrity -> fsync/commit durable state -> return ACK -> fetch from another process -> reconstruct original bytes -> stop a replica -> detect loss -> repair from surviving copies -> restart and verify again.

### C. Sites and Browser
Publish content -> sign/version manifest -> register name/ID -> discover from a different node -> fetch remote content -> verify manifest/content hash -> enforce secret-resource authorization -> update name without changing resource ID -> reject stale/forged records.

### D. ONECOIN / ONEBANK
Validate sender signature, nonce and balance -> create transaction -> distribute proposal -> verify validator set and votes -> verify quorum certificate -> apply candidate block atomically -> persist finalized state -> restart -> reconcile sender and recipient balances -> reject replay/duplicate transaction -> calculate ONEBANK fee exactly once. Fiat settlement remains external/manual; do not report an on-chain fiat transfer.

### E. Admin and shared cloud resources
Authenticate admin -> enforce explicit role grant -> create resource allocation -> validate capacity/quota -> provision isolated workload/storage -> track usage and health -> reject cross-tenant access -> update/revoke permission -> safely stop/delete resource -> retain audit trail. This is a required product track, not a capability assumed to exist merely from tier enums.

## CI and evidence protocol

Run against the exact candidate commit and capture links/output for:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo test --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo audit` (or the configured security workflow's exact command)
- JavaScript syntax validation for every shipped JS entry point
- process-level smoke tests, including a multi-node remote-storage assertion
- supported platform package builds and clean-machine launch/install tests

A cancelled run is not a pass. A formatting pass is not a test pass. A green YAML syntax check is not product validation. When a test fails, record the first meaningful failure, root cause, fix, regression test, and rerun result.

## Scope boundary between the three phases

- **Phase 1 — reconnaissance/documentation:** inventory, architecture, current branch/PR/CI state, risk register, implementation map, ordered plan. This is the purpose of these documents.
- **Phase 2 — backend:** repair and complete real runtime/API/persistence/security/network/storage/identity/admin/node/server/data-centre/data-group/centre-group/ONECOIN/ONEBANK behavior. Verify real node-to-node paths, not only unit structures or YAML.
- **Phase 3 — desktop UI:** re-read Phase 2 changes and contracts first, then build the full native UI without Tauri or embedded web technologies, starting with AWENET-wide Search/Browser, then clear navigation for sites, files, Messenger, Store, identity, node, cloud management, ONECOIN/ONEBANK, and admin. Every UI action must call a tested backend contract and show truthful state/errors.

## Phase 1 exit checklist

- [x] Capture the exact main baseline and full tree inventory.
- [x] Map major source files, applications, platform shells, and workflows.
- [x] Record current open PRs and observed failed/passing workflow categories.
- [x] Record product invariants and readiness evidence rules.
- [x] Create ordered Phase 2 validation and implementation tracks.
- [ ] Run every build/test locally and perform physical multi-node validation — **not a Phase 1 documentation gate; required during Phase 2 and not claimed completed here**.
