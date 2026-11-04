# AWENETwork & founder economy upgrade — 2026-11-04

This upgrade implements the ONECOIN economy directive inside the deterministic
core: founder-exclusive control, the AWENETwork work platform, consented
resource contribution with reliability-weighted rewards, official price
control and treasury accounting. Everything is integer-only, uses explicit
tick/unix time, keeps bounded collections, and is covered by unit tests that
run in CI on every push.

## New modules

### `founder.rs` — FounderAuthority
- Installed exactly once with the founder identity; the founder key is immutable afterwards.
- `authorize(actor_public_key, action, now_unix, purpose)` accepts only the founder key and only for the requested `FounderAction`.
- Every authorization is appended to a bounded audit log (max 10,000 entries); the log is part of the serialized state.
- No profile field or role flag can grant founder rights; they live only in this authority.

### `awenetwork.rs` — WorkMarket
- Founder-only: publish, draft, modify, close and cancel official tasks; approve community suggestions; review submissions; approve-and-pay.
- Anyone may suggest a task; a suggestion never appears in the task market until the founder approves it.
- Contributors accept a task, submit evidence-backed work, receive revision requests and resubmit; rejection requires a reason and mints nothing.
- `approve_and_pay` authorizes via FounderAuthority and mints through `OnecoinLedger::mint_approved_reward` with the deterministic payout id blake3(task || submission || contributor); the same submission can never be paid twice.
- `approve_and_pay_team` is allowed only when the task allows teams; the split must be non-empty and sum exactly to the published reward.
- Phases carry their own rewards, which must sum to the task reward.
- Contributor profiles and levels (Beginner, Contributor, Advanced, Expert, Master) are derived only from real approved work and never mint ONECOIN.

### `resource_economy.rs` — ResourceEconomy
- Participants grant ResourceLimits (CPU, GPU, RAM, storage, bandwidth, uptime caps) and can update or withdraw consent at any time; withdrawn grants cannot be used.
- Settlement requires UsageEvidence from at least 2 of 3 independent signals; effective usage is the minimum of the reported signals, so a single inflated report cannot inflate the reward.
- Rewards follow effective usage x rate x reliability, with a reliability penalty per limit violation (floor 1000 bps) and usage clamped to consented limits.
- The availability reward is separate from usage pay and is never confused with it.
- Settlement is idempotent: the settlement id is blake3(node || period) and the ledger reward idempotency guarantees exactly-once payment.

### `price_control.rs` — OfficialPriceControl
- Only the founder can set the official ONECOIN price, with an effective time and a public reason.
- The append-only history keeps the old price, the new price, the timestamps and the founder identity of every change.
- The module publishes the standing disclaimer that the internal official price does not guarantee exchangeability at the same value outside the network.

### `treasury.rs` — Treasury
- Tracks issuance vs circulation vs escrow-held amounts, separates task rewards from resource rewards, and reconciles against the ledger total issued atoms.
- Daily, weekly and monthly period statistics plus inflation-risk bands (Low below 500 bps, Medium below 2000 bps, High otherwise) give the founder an honest economic picture.

## Ledger extension in `onecoin.rs`: mint_approved_reward
- Verifier-gated: only an authorized reward verifier (the founder) may mint.
- Idempotent by reward id: a reward purpose pays exactly once (tracked in rewarded_purposes).
- Minted amounts always increase total_issued_atoms; transfers and store sales never pass through this path, so buyer-to-seller payments are never mistaken for new issuance.

## Directive mapping

| Directive section | Implementation |
| --- | --- |
| Founder-exclusive control | founder.rs, verifier-gated minting, founder-only task publishing, review and price |
| Resource contribution | resource_economy.rs |
| AWENETwork | awenetwork.rs |
| Official price | price_control.rs |
| Ledger and treasury | treasury.rs, mint_approved_reward |
| Fraud prevention | idempotent payouts, multi-signal evidence, exactly-once settlement, audit logs |
| Contributor motivation | profiles, levels, earned history; no minting for passive actions |

## Explicitly not yet implemented

The following parts of the directive are not implemented yet and are not
claimed as working:

- AWENET Node runtime wiring that measures real CPU, GPU, RAM, bandwidth and uptime usage and feeds ResourceEconomy.
- Distributed multi-node verification of resource measurements; the core accepts multi-signal evidence but the signals currently come from the caller.
- Founder account protection with Passkeys/WebAuthn and MFA at the product level.
- Desktop UI screens for AWENETwork, wallet QR payments and store dispute handling.
- Storefront escrow integration between escrow.rs and the new payout paths.

## Testing

36 new unit tests across the five new modules plus the ledger idempotency
test run in the existing CI matrix (cargo test --workspace on Linux and
Windows, with fmt and clippy gates).

Control-plane counters fail closed on identifier exhaustion rather than
wrapping, and founder snapshot temporary files use private permissions on Unix
before atomic publication. These protections prevent sequence collisions and
reduce exposure of signed administrative state during writes.
