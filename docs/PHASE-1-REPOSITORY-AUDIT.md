# Phase 1 — AWENET repository audit and implementation notes

**Audit snapshot:** branch `codex/fix-ui-call-handler-parse`, inspected commit `1cb11e5b101f0e7a5af609c7db2b9494ec7374eb` (2026-10-13).  
**Purpose:** establish what exists, what is actually connected, what is only a model/scaffold, and the order of work before Phase 2 (backend) and Phase 3 (native desktop UI).  
**Status:** repository inventory and architecture/product-path first-pass audit recorded. This is not a claim that the project is production-ready or that every source line has passed a complete manual line-by-line review. Phase 1 must remain open until the remaining source-level review and findings are reconciled.

## 1. Product target and naming decisions

AWENET is the network/product name. It is not merely a P2P chat app or a list of disconnected features. The target is a shared network in which independently operated nodes contribute explicitly selected resources and can host/search/open real resources by stable ID.

The intended hierarchy is:

`AWENET → Centre Groups (CG) → Data Groups (DG) → Data Centres (DC) → Nodes`

The exact ownership/membership semantics must be made explicit before backend implementation; the current repository's conceptual topology docs use `Node → Data Centre → Data Group → Centre Group → AWE Net`, while the product requirement above describes AWENET as the top-level network. Use one canonical hierarchy in code, API, UI, and documentation. Names/aliases must not replace cryptographic IDs.

Core product areas: user identity/UID/AWEID; node lifecycle; CG/DG/DC membership and links; operator/admin authority; search/browser; sites/hosting; distributed storage/replication/repair; Messenger/calls/groups/channels; AWESTORE; ONECOIN/ONEBANK; security, policy, observability, install/update and recovery.

## 2. Repository inventory (101 tracked paths at snapshot)

### Root, package, CI and release
- `.gitignore`, `Cargo.toml`, `Cargo.lock`, `LICENSE`, `README.md`
- `.github/workflows/core.yml`, `cross-platform.yml`, `format.yml`, `security-audit.yml`
- `awenet.cmd`, `awenet.ps1`, `awenet.sh`
- `bin/windows-x86_64/awe-node.exe` (prebuilt binary; binary provenance/reproducibility must be verified)

### Desktop app / UI / node API
- `apps/awe-app/README.md`, `package.json`
- `apps/awe-app/src-tauri/Cargo.toml`, `build.rs`, `src/lib.rs`, `src/main.rs`, `tauri.conf.json`, `tauri.android.conf.json`, `icons/icon.ico`, `icons/icon.png`
- `apps/awe-desktop/ui/index.html`, `app.js`, `style.css`, `onecoin.js`, `onecoin.css`, `vendor/qrcode.js`
- `apps/awe-node/Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/native_ui.rs`, `tests/product_smoke.rs`

### Core protocol modules
- `core/Cargo.toml`, `core/README.md`, `core/src/lib.rs`
- `access.rs`, `awenet.rs`, `calls.rs`, `canonical.rs`, `crypto.rs`, `data_plane.rs`, `defense.rs`, `diagnostics.rs`, `discovery.rs`, `federation.rs`, `governance.rs`, `host.rs`, `host_directory.rs`, `identity.rs`, `lan_mesh.rs`, `limits.rs`, `messenger.rs`, `messenger_runtime.rs`, `namespace.rs`, `network.rs`, `network_topology.rs`, `node.rs`, `onebank.rs`, `onecoin.rs`, `onecoin_consensus.rs`, `onecoin_consensus_runtime.rs`, `onecoin_governance.rs`, `onecoin_payment.rs`, `onecoin_store.rs`, `onecoin_wallet.rs`, `permissions.rs`, `policy.rs`, `product.rs`, `protocol.rs`, `readiness.rs`, `recovery.rs`, `registry.rs`, `repair.rs`, `replay.rs`, `replication.rs`, `reputation.rs`, `routing.rs`, `sandbox.rs`, `security.rs`, `storage.rs`, `store.rs`, `supervisor.rs`

### Architecture, product, readiness and test documentation
- `docs/AWENET-ARCHITECTURE.md`
- `docs/AWEP2P-DAILY-STATUS.md`
- `docs/AWEP2P-ENGINEERING-CHARTER.md`
- `docs/PRODUCT-ARCHITECTURE.md`
- `docs/PRODUCT-READINESS-GATES.md`
- `docs/REAL-NETWORK-TEST.md`
- `docs/awe-net-production-readiness.md`
- `docs/network-topology.md`

### Clients/platforms/examples
- `clients/README.md`
- `platforms/README.md`
- `platforms/android/AndroidManifest.xml`, `AweNodeService.kt`
- `platforms/aweos/README.md`, `integration.rs`
- `platforms/linux/daemon.rs`
- `platforms/windows/service.rs`
- `examples/awe-host/index.html`

## 3. Architecture as implemented today

### Rust workspace
- Workspace members are `core` and `apps/awe-node`. The Tauri shell is a separate nested Cargo package under `apps/awe-app/src-tauri`.
- `core/src` contains many meaningful protocol/security/data-model modules: identity and vault, encrypted peer transport, replay/limits/defense, discovery/routing, storage and shard messages, namespace/registry, site hosting, Messenger, ONECOIN/ONEBANK, topology/federation, Store, diagnostics and recovery.
- Presence of a module/model does not establish that the feature is wired end-to-end. The repository's own readiness docs correctly state this distinction.

### Running product path
- `apps/awe-node/src/main.rs` is a large (~148 KB) monolithic runtime and HTTP API router.
- The API serves the shared web UI from `include_str!` assets and defaults to local UI address `127.0.0.1:41800`.
- Tauri config uses `frontendDist: ../../awe-desktop/ui`; `apps/awe-app/src-tauri/src/lib.rs` spawns a bundled `awe-node` process but does not retain/manage its child handle or prove graceful shutdown/restart.
- A separate eframe/egui native UI exists in `apps/awe-node/src/native_ui.rs`, but the current Tauri app launches the HTML/CSS/JS WebView UI. It is not currently a clean native desktop application as requested for Phase 3.
- The HTML UI is one SPA-style page that swaps DOM content. It is not a full network-wide Google-like search engine yet.

## 4. High-priority findings (confirmed from the inspected code/config)

### P0 — identity, authorization and administration
1. The current UI's “owner profile” display name is saved in localStorage and defaults to a hard-coded personal display name. This is a device-local label, not a cryptographically verified account/profile.
2. Backend `/api/node` exposes node UID/AWEID/username and runtime metadata, but the inspected API router has no administrator role model, no founder claim/initialization ceremony, no authenticated admin session/token, and no admin authorization middleware. Searching the router found no `admin`, `role`, `authorization`, `bearer`, `admin_token`, or `admin_key` markers.
3. Governance/registry modules and the protected `.awea` namespace are primitives, not a complete user/admin access-control system for node, DC, DG, CG, sites, users or network operations.
4. CORS origin checks are useful browser isolation but are not authentication. A local process or direct HTTP client can bypass browser-origin restrictions. All state-changing local API operations need an explicit local IPC/authentication boundary and authorization checks.
5. Founder privileges must never be granted by a name, localStorage flag, public AWEID alone, or client-side UI. Use a one-time local bootstrap protected by the identity vault, a signed owner record, persistent role assignment, and explicit signed delegation/recovery policy. Test that a second identity cannot claim founder/admin.
6. **Critical secret-at-rest issue:** the normal `run_product()` first-run path writes `data_dir/node.awesecret` directly as JSON. `AweSecret` includes a plaintext `secret_key` field. This is not the password-protected `LocalVault` path. Before first real-user onboarding, replace this with encrypted vault creation, password/recovery setup, strict file permissions and migration handling; never silently overwrite or regenerate an existing identity.
7. The federation `join_token` is deterministically derived from public node/DC IDs and validation only checks that it is non-empty. It is therefore not a secret invitation and does not authenticate an owner. `.awenode/.awedc/.dgc` configs need issuer signatures, freshness/sequence, scope, owner authorization and replay-safe join acceptance.

### P0 — false-success and missing end-to-end network behavior
6. `/api/connect` returns `202 Accepted` after spawning an asynchronous bootstrap attempt. The UI currently toasts “Bootstrap complete” immediately, which overstates success. UI must distinguish queued/connecting/connected/failed and use observed peer state.
7. Storage/data-plane protocol objects and local storage code exist, and runtime has peer transfer handlers. However the product-readiness docs explicitly state that remote upload/download, durable acknowledgements, reconstruction, repair, multi-hop and clean-machine behavior still need independent-node acceptance evidence. Never label an object “replicated” solely because a local request was accepted.
8. The Browser UI can retrieve a file by local API file ID and can pass HTTP(S) URLs to an iframe, but arbitrary `awe://` IDs are rendered as explanatory placeholder content; there is no demonstrated network-wide search index/query pipeline or remote site-by-ID resolver in the UI path.
10. Group/channel UI and API endpoints exist, but must be verified for durable identity-bound ownership, member authorization, cross-node delivery, remote acknowledgement, replay/deduplication and restart persistence. A UI list or local JSON state is not proof of a distributed group service.
11. Call signaling/WebRTC UI exists, but real NAT traversal, relay, remote encrypted media, and independent-device audio/video acceptance remain unproven. Do not present these as production-ready.
11. `core/src/sandbox.rs::WasmSandbox::execute_module` does **not execute WASM**: after validating the header/size, it returns a success string describing a simulated execution frame. The capability set and instruction limit are not enforced by an actual runtime. This is a concrete false-capability implementation; Store must not advertise/run packages as sandboxed WASM until a real runtime is integrated with enforced memory/fuel/capability limits and hostile-module tests.

### P0 — ONECOIN / ONEBANK correctness
11. The repository contains ONECOIN ledger, consensus/finality, persistence, governance, payment and wallet modules, plus ONEBANK tier/contribution structures. The current HTTP wallet send path directly mutates a local in-memory ledger and writes it to a local file; it attempts to deliver to a connected peer, but that is not itself proof of network-wide consensus/finality.
12. The current wallet API reports contribution verification as false and reward status as requiring a signed usage receipt. This is honest and should remain so until a trusted, replay-safe receipt verification path is implemented.
13. UI tier/reward copy must not imply guaranteed monthly rewards, actual cloud resources or verified resource contribution until the corresponding signed accounting and resource scheduler are connected and tested.
14. ONECOIN consensus runtime/validator membership, persistent finalized state, transaction gossip, fork handling, quorum safety/liveness and restart/recovery need an integration audit before treating ONECOIN as a production network currency.
15. `core/src/onecoin_consensus_runtime.rs` and `onecoin_consensus.rs` provide consensus primitives, but the current wallet HTTP route directly applies a transfer to a local `OnecoinLedger`. The audit has not found evidence that this API path waits for a quorum certificate/finalized-state transition before reporting acceptance. Treat wallet transfers as local/experimental until that integration is demonstrated.

### P1 — topology and cloud semantics
15. The topology model and federation configuration types describe node/DC/DG/CG groupings, but configuration generation/import is not the same as authenticated membership, live control plane, access policy or operational cloud orchestration.
16. The visible UI calls a Data Centre “Node registered” when a node descriptor exists. This conflates node identity with DC membership and must be removed.
17. UI currently includes a federation render branch that is not exposed by a corresponding visible navigation button in `index.html`; users can be unable to reach a defined page normally.
18. A cloud-like service requires resource accounting, tenant/site ownership, quotas, scheduling, durable placement, health/repair, DNS/name resolution, TLS/HTTPS ingress, logs/metrics, backups, isolation, upgrades and abuse controls. The present repository is an experimental foundation and does not yet prove AWS/GCP-level capabilities.

### P1 — UI, navigation and product identity
19. The current UI has a dashboard-first page, not the requested Search-first home page.
20. Navigation includes Home, Browser, Messenger, Groups & Channels, Files, Store, ONECOIN, My Node, Network, Security, Developer and Settings. Some pages are shallow summaries, duplicated, unreachable or not backed by a full feature path.
21. `app.js` uses a hand-built HTML-string renderer with scattered event wiring. The single large backend router and UI renderer make it difficult to maintain a strict API contract.
22. `index.html` is a WebView UI. Tauri `tauri.conf.json` sets `csp: null`; this should be replaced by a deliberate restrictive CSP after the Phase 3 native-UI migration is planned.
23. Product identity is inconsistent: repository is AWENET, but README, Cargo package/product names, UI metadata, Android labels and native shell still say AWEp2P in multiple places. Launch scripts also fetch a “latest” package from the old `ARARAT33/AWEP2P` repository. Brand/package/release migration must be deliberate and consistent.
24. Tauri desktop setup starts the bundled node binary via `Command::spawn` without keeping the child handle; shutdown, duplicate start, crash recovery, process ownership and port collision handling need implementation.
25. UI's “Upload & Replicate” wording is stronger than the acceptance evidence currently established. Product copy must reflect actual observed state.

### P1 — release engineering and test evidence
26. `cross-platform.yml` builds Windows/Linux/macOS desktop bundles and Android APK/AAB. Build artifacts alone do not prove installation, lifecycle, network joining or app behavior.
27. The Linux artifact workflow was adjusted in commit `1cb11e5b101f0e7a5af609c7db2b9494ec7374eb` to upload only `.deb`, `.rpm`, and `.AppImage`, avoiding AppImage bundle intermediates. This change still needs a subsequent Actions run and artifact-size verification.
28. `format.yml` can auto-commit formatting on `main`; avoid CI bots racing against review branches and ensure formatting never masks functional validation.
29. Launch scripts currently reference old AWEP2P releases and can download binaries unrelated to this repo's exact source revision. Fix release URLs/versioning and verify checksums/signatures before use.
30. A prebuilt `bin/windows-x86_64/awe-node.exe` is committed. Its provenance, source revision, build flags, signature/checksum and reproducibility need to be documented or the binary should be handled as a release artifact rather than an unexplained source file.
31. The runtime's first-run identity file and the separate `LocalVault` API are inconsistent: the product auto-creates an unencrypted secret file rather than requiring a password-protected vault. This blocks safe first-run onboarding and must be fixed before admin bootstrap is added.
32. The UI's browser iframe accepts arbitrary HTTP(S) URLs. External content in a WebView needs a strict origin/navigation policy and isolation; it must not share privileged local API authority or identity data with untrusted pages.
33. The ONECOIN UI formatter displays at most nine fractional digits, while the core ledger uses 18 decimal atomic units. The UI must either show the full supported precision or clearly round only for display without affecting transaction inputs.
34. The backend `parse_onecoin_atoms()` currently rejects amounts with more than 8 decimal places even though the core ledger defines 18 decimal atomic units. The wallet UI and API therefore disagree with the protocol precision; define one canonical decimal precision and test boundary values before money-transfer UI is rewritten.
35. In `/api/federation/import`, malformed JSON deserialization uses `?` inside the request handler's outer result context in some arms. A parse error can escape the route instead of returning a structured 4xx JSON response, so import failures may surface as a dropped/failed request. Convert all malformed-input paths into explicit API error responses and add malformed/truncated config tests.
36. `/api/storage/put` writes the manifest directory/file with ignored filesystem errors (`let _ = fs::create_dir_all` and `let _ = fs::write`) but can still return a successful/stored response. A later download/repair may fail because durable metadata was never committed. Manifest commit must be atomic and its failure must fail the operation or enter a recoverable explicit state.
37. HTTP request parsing is a hand-rolled server. It handles Content-Length with a body-size cap but does not implement chunked transfer encoding and currently defaults an unparsable Content-Length to zero. Tighten malformed/duplicate Content-Length handling, reject unsupported transfer encodings, and test partial headers/bodies, oversized requests, and slow clients.
38. **Critical peer transport asymmetry:** `Node::listen()` spawns `Node::handle()` for inbound connections, but `handle()` is not passed the node's `active` connection map and never inserts the accepted `SecureConnection` there. The outbound `send_to_peer()` path uses active connections populated by `bootstrap()`. A node that only received an inbound connection can therefore list a known peer but lack a usable outbound session for application replies. Test both directions independently and fix connection ownership/lifecycle before relying on messaging, group delivery, storage transfers or network control.
39. Transport-level `DataAck` means the remote transport accepted a packet into its bounded inbox; it does not prove that the application handler validated, persisted, or acted on it. Application delivery/storage/consensus acknowledgements must be distinct and tied to durable state transitions.
40. `GovernanceEngine::authorize_action` currently does not verify `GovernanceCapability.signature`, issuer/holder key bindings or actor identity; it only checks expiry and string scope. This is a security-critical authorization primitive bug and must be fixed before any admin/DC/DG/CG permissions are built on it.
41. Federation config `join_token` generation is deterministic from public IDs, and `validate_awenode/awedc/dgc` only checks it is non-empty. Any party can produce an apparently valid config; it must not grant membership or administration until the issuing owner signs the invitation and the join flow verifies it.
42. `onebank::classify_tier` maps raw `ResourceContribution` claims (including `server_count`) directly to tiers such as DATA GROUP and CENTRE GROUP. The HTTP contribution endpoint accepts those fields from the caller and reports `verified: false`, yet wallet UI displays the resulting tier. A user can self-claim huge resources and get a misleading high tier; roles/management permissions must be independent of tiers, and tier/reward eligibility must use verified signed receipts only.
43. `core/src/product.rs::ResourceManifest::from_bytes` sets `encrypted: false` and the current storage HTTP path shards raw input bytes. The code must not imply private-by-default distributed storage; access mode, encryption, metadata leakage and key recovery need explicit acceptance tests.

## 5. API surface observed in `apps/awe-node/src/main.rs`

The router includes static UI assets and these API families:
- `/api/node`, `/api/status`, `/api/health`, `/api/policy`, `/api/connect`
- `/api/federation`, `/api/federation/generate`, `/api/federation/import`
- `/api/storage`, `/api/storage/put`, `/api/storage/push`, `/api/storage/get`, `/api/storage/health`
- `/api/messenger`, `/api/messenger/send`, `/api/call/signals`, `/api/call/signal`
- `/api/groups`, `/api/groups/create`, `/api/groups/send`
- `/api/channels`, `/api/channels/create`, `/api/channels/subscribe`, `/api/channels/publish`
- `/api/store/catalog`, `/api/store/install`, `/api/security`
- `/api/onebank/wallet`, `/api/onebank/contribution`, `/api/onebank/wallet/send`, `/api/onebank/exchange/offers`

No dedicated API family was found for first-run account bootstrap, user directory/profile, roles/admin, node provisioning, DC/DG/CG administration, network-wide search, domain/site lifecycle, tenant/cloud quotas or authenticated admin actions. These are backend Phase 2 deliverables, not merely UI pages.

## 6. Validation already represented in repository (not independently rerun by this audit)
- CI defines `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo test --workspace --all-targets`, product smoke test, and `cargo clippy --workspace --all-targets -- -D warnings`.
- `apps/awe-node/tests/product_smoke.rs` launches three independent local node processes and checks health/status, storage, and node connection paths. Its existence is good, but each advertised capability still needs direct assertions and negative/security tests.
- `core/src` contains unit tests across identity, storage, network, ONECOIN, policy, topology, messaging, registry and other modules.
- This documentation commit itself is not a build/test result. Do not infer CI status from YAML contents.

## 7. Remaining Phase 1 checklist

- [x] Inventory all 101 tracked paths at the audit snapshot.
- [x] Read top-level product docs, manifests, CI workflows, desktop entry points, UI route/render structure, major backend router/API families, identity/ONEBANK/ONECOIN/topology/storage/network module contracts.
- [x] Record the highest-risk mismatches and phase boundaries in this document.
- [x] Extend targeted backend audit through request framing, federation import, groups/channels, Store, storage upload/repair, runtime identity initialization, sandbox execution, wallet amount parsing and inbound/outbound peer connection lifecycle; record confirmed defects above.
- [ ] Finish line-by-line review of the complete 148 KB backend router and all request parsing, persistence, authorization, error and shutdown paths.
- [ ] Finish line-by-line review of every core module, tests and platform integration; distinguish tested logic from unused scaffolding and identify concrete bugs with file/line evidence.
- [ ] Reconcile all endpoints with UI actions (request → validation/auth → state transition → persistence → response → visible UI state) and document each gap.
- [ ] Run the full workspace tests, clippy, formatter, JS syntax and three-node smoke on the exact commit; record actual run URLs/results.
- [ ] Resolve architecture naming/hierarchy and decide the authoritative model for founder/admin, user UID/AWEID, node, DC, DG and CG before Phase 2 API contracts are frozen.
- [ ] Complete the findings register with severity, evidence, acceptance tests and fix order.

## 8. Phase 2/3 implementation guardrails

1. Do not start by adding UI-only admin badges. Build authorization and persistent role checks into the backend first.
2. Every privileged operation must be denied by default and require verified role/capability; test unauthorized, expired, replayed and wrong-owner requests.
3. Keep user AWEID/UID, node identity, DC/DG/CG IDs, role assignments and resource IDs distinct and immutable; names are mutable aliases only.
4. Separate control plane (membership, governance, scheduler, health) from data plane (content transfer, storage, site responses).
5. Every API contract must define request schema, validation, authentication, authorization, persistence, error codes, idempotency and UI state transition.
6. Every feature marked live must be tested against at least two independent node processes; distributed claims need independent-process/device tests.
7. Phase 3 UI must be a genuine native desktop UI without Tauri/WebView, Search-first, simple navigation, and fully connected to the Phase 2 API. No fake search results or decorative topology.
8. Do not claim “100% ready” until all checkboxes have repeatable evidence on the exact source revision and clean-machine install tests pass.

## 9. Authoritative status

**Current classification: experimental foundation, not a complete AWENET cloud/network product.** The audit found real protocol and runtime code, but also major missing pieces in authorization/admin, search and site resolution, topology membership/control, resource orchestration, distributed persistence/finality, and native desktop integration. Phase 1 must close the remaining checklist above before declaring it complete.
