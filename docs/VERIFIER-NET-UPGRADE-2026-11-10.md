# Multi-node usage verification — 2026-11-10

This upgrade closes the multi-node usage verifier gap that the economy
completion (2026-11-05) and economy product layer (2026-11-06) documented
honestly as open: settlement requires at least two distinct verifier
signatures, but only single-node reports existed. `core/src/verifier_net.rs`
gives that requirement its network half. No existing module, rule, rate or
payout behaviour changed; the new layer composes with the existing
`resource_meter` and `resource_economy` exactly as designed.

## What existed before

- `resource_meter.rs` already defines `SignedUsageReport` (a verifier's
  Ed25519-signed measurement of one node's usage in one period) and
  `UsageVerification`, whose `consensus_usage` requires
  `MIN_INDEPENDENT_VERIFIERS` (2) distinct valid verifiers agreeing on the
  same node and period and pays the per-resource minimum across their
  reports.
- `resource_economy.rs` settles a period from multi-signal
  `UsageEvidence`, where the network measurement is the verifier
  consensus; no consensus means no network signal, and a single
  inflated report can never raise a reward.
- What was missing: nothing enrolled verifiers, nothing opened a
  bounded attestation window per settlement period, and nothing carried
  a peer's signed report from one node to another. In a real deployment
  the settlement path could collect at most the node's own signals.

## What this adds

### 1. Founder-gated verifier registry

`VerifierRegistry` is installed once with the founder identity, exactly
like `FounderAuthority`. Only the founder key can enroll or retire
verifier nodes, and every enrollment is a founder-signed
`VerifierCertificate` (monotonic sequence, enrollment/retirement
timestamps, Ed25519 signature over the canonical unsigned form). Any
peer can validate a certificate offline against the founder public key;
a certificate whose claimed AWE ID does not derive from the verifier's
public key is invalid even when the signature verifies. There is no
path — in this module or the API — for another user, administrator or
AI agent to enroll verifiers or acquire the founder right.

### 2. Attestation windows

A contributing node opens one `AttestationWindow` per settlement
period: bounded period length (max 30 days), a report deadline
(period end + 24 h grace), one window per (node, period) and a bounded
window registry (4 096). `submit_report` enforces every acceptance
rule before a report can ever count towards a settlement:

- the report's Ed25519 signature must verify (tampered reports are
  rejected, never silently accepted);
- the window must exist for exactly this node and period and still be
  open (late reports are rejected after the deadline);
- the verifier must be enrolled and active (retired or unknown
  verifiers are rejected);
- a node cannot verify its own contribution (self-attestation is
  rejected even if the node is itself enrolled as a verifier);
- the same verifier cannot report twice for one window;
- storage is bounded (16 reports per window, matching
  `MAX_VERIFIER_REPORTS`).

`close_expired` closes windows past their deadline, keeps the
collected reports for an audit retention horizon (30 days) and then
prunes them.

### 3. Honest peer observations

`PeerObservation` is what one node can honestly measure about another
node over the network: the bytes the pair exchanged and the time the
node answered direct contact. `into_report` turns an observation into
a signed `SignedUsageReport` that carries only those remotely
observable resources (bandwidth, uptime) and leaves CPU, GPU, RAM and
storage at zero. Because the settlement consensus is the per-resource
minimum, resources no remote verifier can see earn nothing through
this path — a remote verifier never invents counters it cannot read.

### 4. Settlement composition

`VerifierNet::verification` builds the same `UsageVerification` the
resource meter produces, `has_consensus` reports whether the window
reached the required independent verifier consensus, and `evidence`
composes the settlement evidence (node report + verifier consensus +
delivered results). The end-to-end test settles a real period through
`ResourceEconomy::settle` with two distinct enrolled verifiers and
verifies the exact minted amount, the idempotence guard, and that one
verifier alone produces no evidence at all.

## Guarantees kept

- Only the founder manages the verifier registry; backend rule, not a
  UI promise (directive §2, §17).
- Rewards are proportional to real, independently verified usage, and
  the verifier consensus is conservative (minimum per resource)
  (directive §3, §17).
- No mechanism here mints ONECOIN, changes official price, rates or
  payouts; all money movement still goes through the existing
  idempotent, verifier-gated `mint_approved_reward` (directive §9, §17).
- All state is bounded and serializable (round-trip tested).

## Test evidence

Twelve tests in `core/src/verifier_net.rs`, all running in CI on
ubuntu-24.04 and windows-latest:

1. `only_the_founder_can_enroll_verifiers` — imposter signers and
   double enrollment are rejected.
2. `forged_certificates_do_not_validate` — imposter-signed and
   AWE-ID-mismatched certificates are invalid.
3. `retired_verifiers_are_no_longer_accepted` — retirement is
   founder-gated, idempotent, and retired verifiers' reports are
   rejected.
4. `windows_validate_periods_and_open_once` — malformed, over-long,
   expired and duplicate windows are rejected.
5. `unenrolled_and_self_reports_are_rejected`.
6. `tampered_reports_are_rejected`.
7. `duplicate_verifier_reports_are_rejected`.
8. `late_reports_are_rejected_and_windows_close` — deadline
   enforcement, audit retention, pruning.
9. `two_verifiers_reach_consensus_and_settle` — end-to-end settlement
   with exact payout amounts, no-consensus negative path and
   exactly-once settlement.
10. `peer_observations_attest_only_observable_resources`.
11. `registry_is_bounded` / `windows_are_bounded` /
    `windows_bound_their_reports` — every bound is enforced.
12. `verifier_net_round_trips_through_serde` — full state
    serialization round trip, restored certificates still validate,
    restored reports still verify.

## Honestly not implemented

- Node transport wiring: the A2P2 transport messages and the local
  HTTP routes in `apps/awe-node` that would carry enrollment
  certificates, window announcements and reports between live nodes are
  the next increment. The protocol, validation and settlement
  composition in core are complete and tested; wiring them into
  `economy_api` and the network stack is deliberately not claimed here.
- Verifier discovery/sampling policy: how a contributing node picks
  which peers to ask, and how verifiers decide what to corroborate, is
  deployment policy built on this module, not part of it.
- Multi-node marketplace sync remains open, as documented on
  2026-11-05/2026-11-06.
