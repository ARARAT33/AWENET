# Economy completion & founder protection — 2026-11-05

This layer completes the core-side gaps left open by the
[AWENETwork & founder economy upgrade](AWENETWORK-UPGRADE-2026-11-04.md)
against the ONECOIN economy master directive. It adds five core modules,
each with unit tests, and changes nothing about the existing ledger,
economy or network code.

## Modules

### `founder_guard.rs` — founder account protection

The directive requires the founder's account to be protected by
passkeys/WebAuthn, MFA and a safe recovery path, with founder powers
never derivable from ordinary profile fields.

- Passkey credential registry: a credential is enrolled only against a
  founder-signed enrollment, and the registry is append-only per
  credential id.
- MFA step-up: sensitive founder actions require a challenge/response
  step-up session with a bounded TTL (900 s) that is consumed by use.
- Recovery: single-use recovery codes; a used code is refused a second
  time.
- `authorize()` wraps `FounderAuthority`: a step-up-verified founder
  identity is required for the wrapped action, otherwise the underlying
  founder authorization is never reached.

### `resource_meter.rs` — real resource measurement

The directive rewards real contribution, not declared contribution.

- `SystemMeter` trait with `HostMeter` reading real Linux /proc
  counters (CPU ticks, memory, uptime); on other platforms it returns an
  honest `Unsupported` error rather than an invented number.
- `UsagePeriodMeter` turns two counter snapshots into a
  `MeasuredUsage` period delta.
- `SignedUsageReport` + `UsageVerification`: a report settles in the
  resource economy only with valid signatures from at least two distinct
  verifiers; consensus takes the conservative minimum per field, so a
  single overstating verifier cannot inflate a payout.
- End-to-end test: two verifier-signed reports flow into
  `ResourceEconomy::settle` and the reward matches the verified usage.

### `store_escrow.rs` — escrowed store orders over ONECOIN

The AWENET Store pays sellers from buyer funds, never from new issuance.

- Orders go Created → Funded (escrow) → Released/Refunded/Expired; the
  buyer's payment is escrowed, not handed to the seller up front.
- Delivery confirmation is buyer-only; the seller receives the amount net
  of a founder-configurable fee capped at 500 bps.
- Disputes can be opened by buyer or seller; resolution is
  founder-only (`FounderAction::SettleDispute`) with both rulings
  (release or refund).
- Expiry sweeps refund held, undisputed orders.
- Self-dealing (buyer == seller) is rejected.

### `fraud.rs` — advisory fraud detection

The directive's fraud scenarios, as an advisor to the founder with no
autonomous powers.

- Inflated usage reports (tolerance in bps; a claim with no measurement
  behind it is flagged).
- Duplicate payouts (the same reward id presented twice), duplicate
  submissions (the same contributor + content hash), self-dealing orders,
  wash trading (identical buyer-seller orders repeated inside a window)
  and transfer velocity.
- Bounded alerts and bounded evidence observations with content hashes;
  acknowledging an alert is bookkeeping only.
- The detector holds no funds and can neither mint, pay, approve work
  nor suspend accounts; those stay with the founder paths.

### `rankings.rs` — motivation without minting

Rankings, levels and achievements derive strictly from recorded earning
events (resource, work, store revenue) and never mint ONECOIN.

- Validated `EarningEvent` history, bounded at 100,000 events with an
  explicit `HistoryFull` error (no silent drops).
- Leaderboards by category and time window, ranked by atoms with a
  stable AweId tiebreak, capped at 100 entries.
- Per-contributor statistics including weekly continuity buckets.
- Levels (Beginner → Master Contributor) and achievements
  (FirstEarning, FiveContributions, SteadyContributor, ResourceVeteran,
  TrustedSeller, PillarOfCommunity), all derived from verified history.

## Directive mapping

| Directive requirement | Where it lives |
| --- | --- |
| Founder account protected by passkeys, MFA, safe recovery (§2) | `founder_guard.rs` |
| Resource rewards from real, verified usage (§3) | `resource_meter.rs` + `resource_economy.rs` |
| Store escrow: buyer funds held, delivery confirmed, disputes resolved by the founder (§8) | `store_escrow.rs` |
| Store earnings are transfers, not new issuance (§8) | `store_escrow.rs` over the ONECOIN ledger |
| Fraud and abuse scenarios detected, evidence kept, founder informed (§10) | `fraud.rs` |
| Rankings, levels, achievements from real work only, no minting (§5, §12) | `rankings.rs` |
| No reward without founder approval; no one mints arbitrary ONECOIN (§2, §17) | unchanged: `founder.rs`, `awenetwork.rs`, `onecoin.rs` |

## Explicitly not implemented in this layer

Honesty rule from the directive: what is not wired is listed, not
pretended.

- **Desktop UI screens.** The AWENETwork board, wallet screens with QR
  payments, store front and the founder control center screens are UI
  work on top of these core services and are not part of this layer.
- **Browser-side WebAuthn ceremony.** The core verifies passkey-credential
  signatures; the browser ceremony (navigator.credentials) belongs to the
  client app and is not implemented here.
- **Real multi-node verifier deployment.** The verification mechanism
  (≥2 distinct verifiers, conservative min-consensus) is implemented and
  tested; operating real independent verifier nodes is a deployment task.
- **GPU/storage host metering.** `HostMeter` reads real CPU, RAM and
  uptime counters on Linux; GPU and storage utilization are accounted
  through delivered results and declared limits until a portable
  cross-platform meter is available. Non-Linux hosts get an explicit
  `Unsupported` error, not a placeholder number.
- **Storefront product catalog & search.** `store_escrow.rs` implements
  the order/escrow/dispute lifecycle over the ONECOIN ledger; the
  storefront catalog, product search and seller pages are UI-layer work.

## Testing

Every module ships unit tests inside the module file; CI runs
`cargo fmt --check`, `cargo test --workspace` and
`cargo clippy -D warnings` on Linux and Windows. Highlights:

- `founder_guard`: enrollment requires founder signature, step-up
  expiry and single-use semantics, recovery codes burn on use.
- `resource_meter`: period deltas compute exact usage atoms; settlement
  requires two distinct verifiers; a lone overstating verifier cannot
  inflate the payout.
- `store_escrow`: full lifecycle including fee-capped payouts, payment
  rollback when escrow cannot open, founder-only dispute resolution and
  expiry refunds.
- `fraud`: every alert kind fires exactly when it should and evidence
  stays bounded.
- `rankings`: validation, bounded history, category/window filtering,
  stable sort, weekly continuity, levels and achievements.
