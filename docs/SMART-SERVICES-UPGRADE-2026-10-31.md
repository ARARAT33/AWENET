# Smart node services upgrade — 2026-10-31

This upgrade adds a fourth product-grade layer to AWENET: **smart node
services**. It follows the same rules as the observability (PR #67) and node
platform (PR #74) layers — deterministic, bounded, replay-friendly modules
with no wall clock and no threads, validated by unit tests and the standard
CI gates.

## New modules

| Module | What it gives you |
| --- | --- |
| `core/src/automation.rs` | Metric-driven automation rules with cooldowns, enable flags and a dry-run `plan()` that previews what would fire without recording it |
| `core/src/ratelimit.rs` | Keyed token-bucket limiter driven by explicit ticks: burst capacity, refill, and `retry_after_ticks` on denial |
| `core/src/firewall.rs` | Prefix-based peer firewall with first-match semantics, per-rule reasons and a configurable default action |
| `core/src/cache.rs` | Bounded LRU cache (max 65,536 entries) with deterministic eviction, non-promoting presence checks and explicit remove/clear |

## Design rules kept

- **Deterministic**: time is an explicit tick counter supplied by the caller;
  runs are reproducible from the same inputs.
- **Bounded**: every structure has an explicit maximum (`MAX_RULES = 128`,
  `MAX_BUCKETS = 1024`, `MAX_FIREWALL_RULES = 256`, `MAX_CAPACITY = 65,536`)
  and returns typed errors instead of growing without limit.
- **Serializable**: engines and rules serialize to JSON, so they can be
  persisted, inspected and replayed.
- **First-class errors**: every module has its own `thiserror` error enum.
- **Tested**: every module ships unit tests covering validation, the main
  behaviour paths, edge cases and round-trip serialization.

## How the pieces fit

- `automation` consumes gauge values the same way `alerts` does (it reuses
  `alerts::ThresholdOp`), but instead of only notifying it emits actions
  (`notify`, `shed_load`, `trigger_backup`, `purge_cache`) with cooldowns.
- `ratelimit` is the natural enforcement point for `shed_load` decisions and
  for `quota`-style allow/deny accounting on request paths.
- `firewall` complements `defense`: `defense` limits hostile traffic shapes,
  `firewall` makes explicit per-peer allow/deny decisions with reasons.
- `cache` is the local building block for resource lookups and `search`
  result memoization, and `purge_cache` automation gives operators a
  deterministic way to flush it.

## Verification

- `cargo fmt --all -- --check`
- `cargo test --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- No protocol or persistence format changed; all additions are new, purely
  additive modules registered in `core/src/lib.rs`.
