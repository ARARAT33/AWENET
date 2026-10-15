# Phase 1 — UI ↔ API contract matrix

Snapshot: `codex/fix-ui-call-handler-parse`, audit base commit `1cb11e5b101f0e7a5af609c7db2b9494ec7374eb`. Companion: [PHASE-1-REPOSITORY-AUDIT.md](./PHASE-1-REPOSITORY-AUDIT.md).

This matrix records current UI behavior, not intended behavior. A screen is not considered implemented merely because it renders or a request returns 2xx.

| UI area | Current UI path / API | Confirmed gap / acceptance requirement |
|---|---|---|
| Home / Dashboard | Periodically calls `/api/status`, `/api/node`, `/api/storage`, `/api/security`, `/api/federation` | Displays a topology illustration from peer count, not actual CG/DG/DC links. “Data centre” card currently infers membership from node descriptor. Replace with observed topology and role-based actions. |
| Search / Browser | `awe://fid-<64hex>` calls `/api/storage/get`; HTTP(S) is loaded into iframe | No network-wide search endpoint/index, no remote site-by-ID resolution path shown. Arbitrary `awe://` IDs display explanatory placeholder HTML. New first page must be Search and show real results only. |
| My Node / User | `/api/node`; display name saved in browser localStorage | Local label only, not a user profile. No persistent user directory, account bootstrap, signed profile, UID ownership registry, node enrollment workflow, resource selection wizard or stop/restart controls. |
| Network | `/api/status`, `/api/connect` | Connect API returns an accepted/connecting state; UI immediately toasts “Bootstrap complete”. Wait for observed authenticated connection and show timeout/failure details. |
| AWENET topology | `/api/federation`, `/api/federation/generate`, `/api/federation/import` | Defined render branch lacks a visible navigation entry in `index.html`. Config import is not signed role authorization or proof of membership. Need first-class CG, DG, DC, node, membership, operator, audit and live-link APIs. |
| Storage / Files | `/api/storage`, `/api/storage/put`, `/api/storage/get`, `/api/storage/health` | Upload serializes file as hex JSON (large overhead); UI says “Upload & Replicate” while remote placement and durable metadata need explicit verified states. Manifest write errors are ignored in the upload route. Search, open-by-ID, access maps, delete, quota and recovery need real lifecycle endpoints. |
| Messenger | `/api/messenger`, `/api/messenger/send` | Runtime message log is memory state in this path; verify restart durability, queue retry, signed identity binding, remote receipt and privacy. The UI displays queued/sent states that must be defined against protocol events. |
| Voice/video calls | `/api/call/signals`, `/api/call/signal` plus browser WebRTC | Uses a public STUN server and browser media APIs. No proven TURN/relay/NAT traversal or independent-device media acceptance. Call signaling delivery and media encryption must be explicit. |
| Groups | `/api/groups`, `/api/groups/create`, `/api/groups/send` | Local JSON persistence loop plus peer event messages. Verify owner/member signatures, remote membership convergence, deduplication, durable writes and non-member rejection on all receiving paths. |
| Channels | `/api/channels`, `/api/channels/create`, `/api/channels/subscribe`, `/api/channels/publish` | Local subscriber mutation can precede owner acknowledgement; event replication needs verified sender/owner, retries, durable state and real subscriber-targeted delivery. |
| AWESTORE | `/api/store/catalog`, `/api/store/install` | Reads locally cached packages and install metadata. No complete remote discovery/publish/update/revoke/download flow demonstrated. UI consent is not runtime capability enforcement. WASM execution is simulated, not implemented. |
| ONECOIN / ONEBANK | `/api/onebank/wallet`, `/api/onebank/contribution`, `/api/onebank/wallet/send`, `/api/onebank/exchange/offers` | Transfer mutates local ledger then attempts peer send; not proven finalized consensus. API amount parsing accepts max 8 decimal places while ledger defines 18. Offers can be saved locally after network publish fails, so UI must distinguish local draft from published. Contribution is correctly marked unverified. QR scan/receive handlers are conditionally wired but the current generated markup has no corresponding QR elements. |
| Security | `/api/security` | Static capability summary, not live audit or admin policy. Needs roles, signed delegation, session auth, local IPC protection, audit events, permission inspection and recovery. |
| Diagnostics | `/api/health`, `/api/status` | Basic probes only; needs real resource metrics, recent errors, peer session state, storage integrity, replication health, version/revision and exportable sanitized diagnostics. |
| Settings | localStorage preferences + identity display | Preferences are local only; no vault/password/recovery management, network profile, node lifecycle, update, backup, or trusted admin actions. |

## Acceptance rule for every control

For every button and form, document and test:
1. UI validation and disabled/loading state.
2. Request schema and endpoint.
3. Authentication and authorization policy.
4. Backend validation and deterministic error code.
5. State transition and durable persistence.
6. Remote effect, if any, with application-level acknowledgement.
7. Exact response semantics (accepted, pending, committed, failed).
8. UI rendering for success, partial success, timeout, retry and restart.
9. Positive, negative, malformed-input and multi-node tests.

## Required UI navigation after redesign

- **Search** (first page): one search field across AWENET resource IDs, sites, people/UIDs, apps and available public metadata.
- **My Space**: profile, stable UID/AWEID, private/open/secret resources, sites, downloads, saved items.
- **Node**: first-run setup, resource contribution controls, health, storage root, start/stop/recovery.
- **Network**: CG → DG → DC → node hierarchy, live authenticated connections, status and routes.
- **Sites / Cloud**: create, publish, map name to immutable site ID, version, domains, access, quota, logs and deployment status.
- **Messenger**: contacts by immutable UID, messages, groups/channels and calls.
- **AWESTORE**: verified package catalog, permissions, install/update/rollback.
- **ONECOIN / ONEBANK**: wallet, finalized transactions, contribution receipts/rewards, P2P offers and collective governance.
- **Admin / Operator**: visible only after backend role verification; user, node, DC/DG/CG and network operations, signed audit log and role delegation.
- **Settings / Security**: vault, recovery, privacy, access maps, limits, updates and diagnostics.

Phase 3 must implement this as a true native desktop interface without Tauri/WebView. This matrix does not authorize beginning Phase 3 before the Phase 1 audit and Phase 2 backend contracts are complete.
