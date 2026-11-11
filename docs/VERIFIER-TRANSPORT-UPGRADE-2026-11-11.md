# Live Verifier Transport Upgrade — 2026-11-11

## Scope

The 2026-11-10 upgrade (`verifier_net.rs`) defined the multi-node usage
verification contract: a founder-gated verifier registry, per-period
attestation windows and the acceptance rules for signed usage reports. It
deliberately stopped before the network half: nothing carried a report from
the verifier node to the contributing node, and the contributing node had no
way to tell its peers a window was open.

This upgrade closes that gap. `core/src/verifier_transport.rs` is the wire
between live nodes, and the `awe-node` product wires it end to end: the
dispatcher handles the new stream 202, a background cycle announces open
windows and delivers due reports, the founder manages the verifier roster
over the HTTP API, and a founder-signed snapshot keeps the verifier net
across restarts.

## What was built

### `core/src/verifier_transport.rs` (new)

- `WindowAnnounce`: the announcement a contributing node sends for one of
  its open attestation windows. It carries no new authority: every field
  restates a window the node already opened, and receivers validate it as
  such (correct version, bounded period, deadline exactly period end plus
  the verifier-net grace).
- `VerifierTransportMessage` (`Announce` / `Report`): the two message kinds
  on stream 202, JSON-encoded and hard-bounded at 64 KiB so a hostile peer
  cannot push unbounded payloads through the transport.
- `VerifierSide`: the verifier-side observation state of one node. It
  tracks announced windows in bounded memory (1,024 windows, oldest-expired
  pruned), accumulates the bytes this node genuinely exchanged with each
  observed node, and builds exactly one `SignedUsageReport` per window once
  the settlement period has ended. A report is attempted once and capped to
  the observation window: a verifier never re-reports the same window, so
  transport retries can never exercise the contributing node's duplicate
  rule.
- `open_window_announces(net, now)`: the windows of a verifier net that
  are still open for reports, ready to be announced.
- Rejections are explicit and typed: a node cannot announce a window for
  someone else, a malformed or expired announcement is dropped, and an
  announcement may not change an immutable period of an already observed
  window.

### `core/src/verifier_net.rs` (additive)

- `VerifierRegistry::verifiers()`: read-only access to the full signed
  roster (active and retired) for the node API.
- `enroll_public(founder, verifier_awe_id, verifier_public_key, at)`:
  enrollment by public node identity alone (the existing `enroll` by full
  `Identity` is unchanged and delegates to the same internals).
- `VerifierNet::enroll_verifier_public` and `VerifierNet::windows()`.

### `apps/awe-node` product wiring

- The dispatcher handles stream 202: announcements update the
  verifier-side observation state; reports go through
  `VerifierNet::submit_report` and persist immediately in the
  founder-signed snapshot. Every allowed inbound message also counts as
  traffic against the sender's observed windows, which is the honest
  remote observation the reports are built from.
- A 15-second `verifier_transport_cycle` announces every open window to
  the connected peers, delivers the reports that became due, and closes
  expired windows through `close_expired`.
- Startup loads `awenet-verifier-net.json` fail-closed: a snapshot that
  fails founder validation is a hard error, never a silent fresh start.
- New HTTP endpoints in `economy_api`:
  - `GET /api/verifier/net` — the verifier roster, every attestation
    window with the reports collected for it and the consensus reached.
  - `POST /api/verifier/reports` — signature-verified report intake, the
    HTTP twin of the live transport.
  - `POST /api/founder/verifier/enroll|retire|window` — founder-only,
    gated by the same backend authority (plus MFA step-up once a passkey
    is enrolled) as every other founder action. Enrollment takes a
    public node key; the AWE id is derived deterministically from it.
- `/api/status` peers now publish their `awe_id` and `public_key` so the
  founder can enroll live peers from the running product.

## Guarantees kept

- Nothing in this layer mints ONECOIN, changes reward rates or prices, or
  settles anything: settlement stays with the resource economy and reads
  the consensus exposed by `GET /api/verifier/net`.
- Every acceptance decision still runs inside `VerifierNet`: enrolled
  active verifier, valid signature, open window, no self-attestation, no
  duplicate, bounded storage. The transport cannot weaken any of them.
- The founder's exclusive rights are enforced in the backend, not in the
  UI: enrollment, retirement and window opening are founder-only routes.
- Reports carry only what a peer can honestly observe over the network
  (bytes exchanged, contact time); no counter a remote peer cannot see
  is ever invented.
- The verifier net persists as a founder-signed snapshot; a corrupted or
  foreign snapshot fails closed at startup.

## Testing

- 8 unit tests in `verifier_transport.rs`: message roundtrip and
  malformed/oversized rejection, announcement well-formedness, foreign/
  malformed/expired announce rejection, traffic attribution, one-shot
  due reports capped to the period, prune behavior, bounded observation
  registry, and a full end-to-end flow from announcement to two-verifier
  consensus.
- 2 tests in `economy_api`: founder-only gating of the verifier routes
  (a stranger gets 403) and the full flow enroll → window → two signed
  reports → duplicate rejection → forged-signature rejection → consensus
  status → founder-signed snapshot reload.
- The three-node product smoke test now enrolls nodes 2 and 3 as verifiers
  by their public keys, opens a 10-second window on node 1 and waits for
  live consensus (`has_consensus` with two reports arriving over stream
  202), then asserts the verifier roster survives a node-1 restart.
- CI runs fmt, the full workspace tests, the product smoke test and
  clippy -D warnings on Ubuntu and Windows.

## Honestly not implemented here

- The verifier-side observations are in-memory by design: they are
  rebuilt from live announcements and nothing about them outlives the
  node's uptime. This is stated, not hidden.
- A report is attempted once per window per verifier node. There is no
  retry queue: if the contributing node is unreachable at the moment the
  report is due, that window's report from that verifier is lost, and
  the settlement simply runs with fewer reports (and may fail its
  two-verifier minimum, which is the honest outcome).
- Traffic accounting counts only verifier-transport-relevant messages the
  dispatcher sees; it is an honest lower bound of the exchanged bytes,
  not a byte-perfect accounting of the TCP stream.
- Settlement automation on the contributing node (feeding a reached
  consensus into a `resource_economy` payout without an explicit founder
  action) is not part of this upgrade; the consensus is exposed and the
  founder decides.
- Multi-node marketplace sync (storefront catalog and orders across
  nodes) remains the last open gap of the economy product layer.
