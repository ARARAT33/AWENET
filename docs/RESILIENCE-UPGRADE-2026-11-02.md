# Intelligence & Resilience Services Upgrade (2026-11-02)

This upgrade adds a fifth deterministic services layer to the AWENET core:
self-protecting behaviour for the node product. Like the earlier
observability, platform, smart-services and trust layers, every module is
pure data: no wall clock, no threads, explicit ticks supplied by the
caller, so identical inputs always produce identical outputs and replays.

## New modules

| Module | Purpose | Key behaviours |
| --- | --- | --- |
| `insights.rs` | Rolling-window metric series | Bounded windows per named series, min/max/mean/last summaries, rising/falling/flat trend, z-score anomaly verdicts with per-series thresholds |
| `capacity.rs` | Capacity forecasting | Headroom in bytes and percent, ticks-to-limit from a growth-per-tick estimate, urgency bands (Ok / Warn / Critical / Full) with concrete recommendations |
| `circuit.rs` | Circuit breaker | Closed / Open / HalfOpen states, consecutive-failure threshold, cooldown ticks, `retry_after_ticks` on denial, probe outcome closes or re-opens |
| `journal.rs` | Write-ahead operation journal | Bounded append-only records with monotonic sequence numbers, ordered replay, suffix recovery via `records_after`, checkpoint truncation |
| `archiver.rs` | Retention & compaction | Age tiering (Hot / Warm / Cold), largest-first archiving within a bounded archive budget, deterministic sorted plans with archived/reclaimed byte accounting |

## Design rules kept

- **Deterministic and replay-friendly** — pure data, explicit ticks, no
  wall clock and no threads.
- **Bounded** — every structure has a hard maximum and validates its
  configuration with typed errors.
- **Evidence over optimism** — verdicts, plans and decisions are returned
  as values the caller can log, audit and test; nothing happens as a
  side effect.
- **No protocol changes** — the layer is additive; existing modules,
  wire formats and platform boundaries are untouched.

## Verification

- `cargo fmt --all -- --check`
- `cargo test --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- New unit tests cover validation, boundedness, state transitions,
  replay/checkpoint recovery and deterministic planning order.

## Integration notes

The modules are wired to be consumed by the scheduler and automation
layers in a follow-up: insights feed anomaly rules, capacity forecasts
drive alerts, circuit breakers gate risky peer operations, the journal
gives crash-safe replay for local state transitions, and the archiver
keeps local stores bounded. As with every layer, real-network evidence
per REAL-NETWORK-TEST.md remains the acceptance bar for distributed
claims.
