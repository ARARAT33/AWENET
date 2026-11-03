# AWENET Phase 1 — Repository Audit and Handoff to Phase 2

**Audit date:** 2026-10-16  
**Audited baseline:** `main` at `e97809f3eb7e0de09b4fa7c2979ac4bcedb0936b`  
**Repository:** `ARARAT33/AWENET`  
**Purpose:** Complete repository reconnaissance before changing backend implementation. This document is an evidence-based inventory and a work contract for Phase 2; it does not claim that AWENET is production-ready.

## 1. Audit result

The default branch contains **127 entries in its recursive Git tree** (files and directories; tree response was not truncated). The project is a Rust workspace with two workspace members, a Rust node runtime, a browser-like UI bundled into the node, and a separate Tauri shell. The codebase already contains substantial protocol and product primitives. Their existence is not proof that every feature is complete end to end.

**Phase 1 outcome:** the repository structure, principal subsystem boundaries, product intent, existing readiness criteria, open work tracks, and currently visible CI state have been recorded. Phase 2 must now validate the implementation path-by-path and test-by-test before adding or replacing behavior. This audit is based on GitHub source, repository tree, open PR metadata, and recent Actions metadata; it is not a claim that the full workspace was compiled or exercised on a physical multi-node network during this audit.

## 2. Product definition and non-negotiable design constraints

AWENET is intended to be one shared network substrate, not a collection of unrelated apps or a conventional single-server P2P product. Browser/search, sites, files, Messenger, calls, groups/channels, Store, node operation, and ONECOIN/ONEBANK are applications/services over the same network and identity/resource rules.

The following requirements must guide later implementation:

- **Identity:** AWEID is the stable cryptographic identity. Human-readable names are aliases, not security principals. Device/node identities and user identity must not be conflated.
- **Resource addressing:** objects need stable cryptographic IDs; names can change without changing the underlying resource ID. Secret resources require authorization; knowing an ID/name must not grant access.
- **Distributed data:** placement, verified durable writes, acknowledgements, discovery, retrieval, replication, and repair must refer to the same resource identity. A UI message must not claim remote persistence until a remote peer has verified and durably stored the data.
- **Network operation:** nodes must be independently runnable processes/devices, with authenticated transport, reconnect, bounded state, failure handling, and observed peer health. Unit tests and local simulations alone cannot establish Internet-wide operation.
- **Resource contribution:** node operators explicitly choose and can withdraw storage, bandwidth, compute, relay, and uptime contribution. Do not silently seize or lock user disks. Any protected storage behavior needs clear consent, recoverability, and operating-system-safe implementation.
- **Privacy/security:** encryption and authentication are not equivalent to anonymity. Never promise absolute anonymity or immunity. Avoid logging secrets, private keys, passwords, or private message contents.
- **ONECOIN/ONEBANK:** use integer atomic units and signed, replay-protected transactions. A local ledger is not global truth; network finality needs a consistent validator set and verified quorum. ONEBANK is a ONECOIN policy/accounting layer, not an in-app fiat custodian; fiat settlement is external/manual per product requirements.
- **Cloud-like management:** node/server, data-centre, data-group, centre-group, user and admin control need explicit ownership, authorization, quotas, resource accounting, lifecycle, auditability, and failure boundaries. A tier enum or dashboard alone is not a functioning cloud control plane.
- **Desktop:** the intended final client is a clean native desktop interface without Tauri or a browser-based shell. The repository currently contains a Tauri shell and an HTML/CSS/JS UI embedded by the node; that mismatch belongs to Phase 3, not to claims about the current UI.
- **Evidence rule:** no feature is complete solely because a module, route, button, workflow, or README exists. Completion requires meaningful tests and reproducible end-to-end evidence tied to the exact revision.

## 3. Repository inventory

### Root and build/release
- `Cargo.toml`, `Cargo.lock`: Rust workspace; members are `core` and `apps/awe-node`.
- `.github/workflows/core.yml`: workspace build/test validation.
- `.github/workflows/format.yml`: formatting gate.
- `.github/workflows/security-audit.yml`: dependency/security audit.
- `.github/workflows/cross-platform.yml`: cross-platform packaging/build workflow.
- `awenet.cmd`, `awenet.ps1`, `awenet.sh`: launch/setup entry points.
- `bin/windows-x86_64/awe-node.exe`: checked-in Windows executable; Phase 2 must verify whether it matches current source/revision before relying on it.
- `LICENSE`, `.gitignore`, `README.md`: project metadata and entry documentation.

### Applications and clients
- `apps/awe-node/src/main.rs`: large runtime/CLI and HTTP API implementation (143,489 characters; 2,523 lines in the audited baseline). It embeds the HTML, CSS, JavaScript, ONECOIN assets, and QR-code library at compile time; defines a default UI listener at `127.0.0.1:41800`; contains node lifecycle, UI serving, product orchestration, repair cycle, and API handlers. Its size makes route/auth/error handling a priority for line-by-line review.
- `apps/awe-node/src/native_ui.rs`: native UI-related Rust module; must be assessed against the target non-web desktop requirement and actual use.
- `apps/awe-node/tests/product_smoke.rs`: process-level product smoke test intended to cover real node processes and storage flows. Its current assertions and coverage must be inspected and run, not inferred from its filename.
- `apps/awe-desktop/ui/index.html`, `style.css`, `app.js`: current HTML/CSS/JS UI. The JS is about 34 KB and wires views to the node's HTTP API. It contains Browser, network/status, storage, federation, groups/channels, Messenger/calls, Store, and related flows. The Browser currently handles HTTP(S) URLs through a frame and has an `awe://fid-<64 hex>` storage-object path; this is not yet proof of Google-like AWENET-wide search or fully remote site hosting.
- `apps/awe-desktop/ui/onecoin.js`, `onecoin.css`: ONECOIN/ONEBANK UI assets.
- `apps/awe-desktop/ui/vendor/qrcode.js`: bundled QR library.
- `apps/awe-app/`: Tauri project shell, configuration, icons, and Android configuration. This is present in the current baseline and must not be mistaken for the desired final native desktop implementation.
- `clients/README.md`, `platforms/README.md`, `platforms/android/`, `platforms/linux/`, `platforms/windows/`, `platforms/aweos/`: platform/client integration scaffolding. Presence does not prove all platform lifecycle/service behavior is production-complete.
- `examples/awe-host/index.html`: minimal host example.

### Core protocol/product modules
The core crate exposes modules from `core/src/lib.rs`. The audited source inventory includes:

| Area | Files | Responsibility / Phase 2 review target |
|---|---|---|
| Identity and authorization | `identity.rs`, `access.rs`, `permissions.rs`, `policy.rs`, `governance.rs` | AWEID/key lifecycle, vault security, ownership and access checks, policy enforcement |
| Network and routing | `network.rs`, `network_topology.rs`, `routing.rs`, `discovery.rs`, `lan_mesh.rs`, `protocol.rs`, `federation.rs` | Handshake/session, sockets, peer records, bounded routing/discovery, federation |
| Data and storage | `data_plane.rs`, `storage.rs`, `replication.rs`, `repair.rs`, `recovery.rs`, `host.rs`, `host_directory.rs` | Content IDs, encryption/sharding, placement, remote transfer, durable ACK, repair/recovery |
| Resource and node lifecycle | `node.rs`, `product.rs`, `supervisor.rs`, `limits.rs`, `readiness.rs`, `diagnostics.rs`, `defense.rs`, `replay.rs`, `reputation.rs` | Node/resource state, supervision, limits, operational health, diagnostics, abuse/replay defense |
| Names and applications | `namespace.rs`, `registry.rs`, `canonical.rs`, `sandbox.rs` | Name/ID resolution, registry ownership/freshness, canonical forms, app isolation |
| Messaging and calls | `messenger.rs`, `messenger_runtime.rs`, `calls.rs` | Message envelopes/delivery and call signaling/runtime |
| ONECOIN/ONEBANK | `onecoin.rs`, `onecoin_wallet.rs`, `onecoin_store.rs`, `onecoin_payment.rs`, `onecoin_consensus.rs`, `onecoin_consensus_runtime.rs`, `onecoin_governance.rs`, `onebank.rs` | Ledger, wallet, store payments, finality/quorum, governance, contribution/tier policy |
| Shared facade | `awenet.rs`, `product.rs` | Network/product abstractions and integration points |

Additional engineering/product docs: `docs/AWENET-ARCHITECTURE.md`, `docs/PRODUCT-ARCHITECTURE.md`, `docs/PRODUCT-READINESS-GATES.md`, `docs/REAL-NETWORK-TEST.md`, `docs/awe-net-production-readiness.md`, `docs/network-topology.md`, `docs/AWEP2P-ENGINEERING-CHARTER.md`, `docs/AWEP2P-DAILY-STATUS.md`.

## 4. Current implementation surfaces found in the node API

The audited `apps/awe-node/src/main.rs` contains handlers for these route families:

- Runtime/status: `/api/node`, `/api/policy`, `/api/status`, `/api/health`, `/api/security`.
- Network/federation: `/api/connect`, `/api/federation`, `/api/federation/generate`, `/api/federation/import`.
- Storage: `/api/storage`, `/api/storage/put`, `/api/storage/push`, `/api/storage/get`, `/api/storage/health`.
- Messaging/communities/calls: `/api/messenger`, `/api/messenger/send`, `/api/groups`, `/api/groups/create`, `/api/groups/send`, `/api/channels`, `/api/channels/create`, `/api/channels/subscribe`, `/api/channels/publish`, `/api/call/signals`, `/api/call/signal`.
- Store: `/api/store/catalog`, `/api/store/install`.
- ONEBANK: `/api/onebank/wallet`, `/api/onebank/contribution`, `/api/onebank/wallet/send`, `/api/onebank/exchange/offers`.

Phase 2 must create a route-to-domain-to-persistence-to-authorization matrix and test every mutating handler. Specifically check method enforcement, input size limits, authentication/authorization, path traversal, JSON validation, concurrency, replay/idempotency, durable state, accurate error codes, and whether the response claims are backed by actual remote evidence. These are review obligations, not assertions that every listed route is currently vulnerable.

## 5. Existing written acceptance criteria

The existing readiness checklist already defines important real-product gates: clean installation; local identity creation and durable restart; independent authenticated node processes; reconnect/discovery/multi-hop bounds; encrypted remote storage; verified durable acknowledgements; remote reconstruction; interrupted transfer recovery; health-driven replica repair; signed/fresh name resolution; remote site fetch; package verification and permission enforcement; acknowledged Messenger delivery; real encrypted media before advertising voice/video; terminal-free desktop launch; reproducible platform artifacts; checksums/signing metadata; clean install/uninstall; and backup/upgrade/rollback.

The real-network test document explicitly limits its direct TCP probe claim and says it does **not** prove DHT, multi-hop, NAT traversal, distributed storage, anonymity, or release quality. Phase 2 must preserve this evidence discipline and extend tests where gaps remain.

## 6. Current Git/CI state observed during audit

- Default branch: `main`, baseline `e97809f3eb7e0de09b4fa7c2979ac4bcedb0936b`.
- Open draft PR #61, `phase-2-backend-hardening`, is the current Phase 2 work track. Its recent commits are focused on confirmed peer data sends/ACKs; recent Actions runs for that branch show **Phase 2 Backend Validation failing**, **AWEp2P Workspace CI failing**, and **Security Audit failing** on the latest observed revision, while **Rust Format Check passes**. This is an immediate Phase 2 starting point, not a reason to declare the backend complete.
- Open draft PR #60 covers UI/security repairs; #59 addresses a fixed A2P2 datagram key; #58 adds readiness evidence; #57 covers ID-addressed site hosting/browser publishing; #55 addresses desktop sidecar lifecycle. These tracks overlap and must be reviewed for conflicts and unmerged fixes before implementation.
- There are multiple additional open feature/fix PRs. Phase 2 should not assume any open PR is already part of `main`; inspect exact base/head SHAs and merge status before depending on code.
- `README.md` describes the project as AWEp2P and explicitly warns that a source module or UI screen is not proof of end-to-end functionality. The requested product name is AWENET; phase work should converge user-facing naming without silently renaming protocol compatibility identifiers or breaking stored data.

## 7. High-priority risks and unresolved questions for Phase 2

1. **Integration is the principal risk.** The node runtime, core modules, UI handlers, and tests must agree on identity, resource IDs, network state, and error semantics.
2. **Confirmed storage semantics.** Distinguish queued, sent, received, integrity-verified, durably written, and replicated. Never report a remote replica on a local enqueue alone. Investigate the latest Phase 2 failing test/logs before changing retry logic again.
3. **Transport/security consistency.** Review the A2P2 static-key issue/PR #59 and ensure datagram keys are session-derived or otherwise correctly scoped, authenticated, rotated, and replay-protected.
4. **ONECOIN consensus/runtime integration.** Inspect validator-set initialization, membership consistency, quorum certificate validation, duplicate votes/proposals, durable finalized state, restart behavior, and network delivery. A locally correct consensus state machine is not enough if nodes disagree on validator membership.
5. **ONEBANK policy semantics.** Verify tier classification, contribution evidence, reward replay prevention, fee handling, sender/recipient ledger updates, and the distinction between P2P ONECOIN transfer and external fiat settlement. A tier name is not permission to administer real resources.
6. **Admin and cloud management.** Audit role assignment and authorization from the backend; ensure no UI-only admin flag. Define lifecycle/quotas/ownership for nodes, servers, data centres, data groups, centre groups, user accounts and identifiers before exposing control panels.
7. **Browser/sites.** Verify that site lookup uses a network registry and remote retrieval with signed/fresh manifests and content integrity. External HTTP browsing inside an iframe is not AWENET-native search.
8. **UI/API errors.** Reproduce the historical “Failed to fetch” class of problems by testing startup, base URL selection, CORS/origin behavior, stopped-node state, request timeout, and each route. Errors must be actionable rather than swallowed.
9. **Persistence and migrations.** Inventory all on-disk state, versioning, atomic writes, corruption recovery, backup/restore, and compatibility before adding new fields.
10. **Build/test coverage.** Run format, check, all tests, clippy with warnings denied, security audit, JS syntax checks, process smoke tests, and supported-platform builds. Capture actual outcomes; do not use YAML validity as a substitute.
11. **Desktop architecture mismatch.** Current HTML UI is embedded in the node and a Tauri shell exists. Do not treat either as satisfying the requested pure native desktop UI.
12. **Scope control.** Avoid large unrelated rewrites until the actual dependency and runtime boundaries are understood. Remove dead/duplicate behavior only after proving it is unused and covered by tests.

## 8. Phase 2 execution order

1. Fetch and inspect the exact latest Phase 2 branch and failed Actions job logs; identify the first failing assertion and root cause before patching.
2. Build a domain/route/persistence/auth matrix for node, user/AWEID, node administration, cloud resource management, sites/browser, storage/replication/repair, Messenger/groups/channels/calls, Store, ONECOIN, ONEBANK, and admin.
3. Trace critical data flows end to end: identity creation -> authentication -> peer handshake -> discovery/connect -> resource upload -> remote durable ACK -> retrieval/reconstruction -> repair; and transaction creation -> signature/nonce/balance validation -> proposal/vote/quorum -> finalized persistence -> wallet/UI update.
4. Fix one root cause at a time, add regression tests, then run the full relevant suite. Avoid changing the same file concurrently across branches.
5. Define and test the backend contracts needed by Phase 3 before redesigning UI: stable routes, typed response/error schemas, auth model, lifecycle semantics, and live status.
6. Update readiness status only with linked tests/runs or reproducible operator evidence. Keep unverified items explicitly open.

## 9. Definition of Phase 1 completion

Phase 1 is complete when this audit and the implementation map are committed, the baseline and outstanding PR/CI state are identified, and Phase 2 has a clear ordered validation plan. It does **not** mean the backend, network, UI, cloud control plane, or production deployment is complete. Those belong to subsequent implementation and validation gates.
