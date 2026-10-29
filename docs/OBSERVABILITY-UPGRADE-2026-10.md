# AWENET observability & automation upgrade (2026-10-29)

This upgrade adds four new core subsystems to AWENET without changing any
existing module. It keeps the architecture of the workspace intact and
follows the engineering charter: deterministic, replayable, std-only where
possible, and fully unit tested in CI.

## Why

The product orchestration layer (`product.rs`, `product_health.rs`) already
models resources, peers, placement, transfers and health. What it lacked was
the supporting operational layer:

1. **Audit trail** — operators need a bounded history of what a node did.
2. **Operational metrics** — the health score needs raw counters and latency
   samples, not only band labels.
3. **Local search** — the Browser and Store screens need to find content on
   the node itself.
4. **Scheduled maintenance** — repair sweeps, backups and health recomputation
   should run on a deterministic, replayable schedule.

## What was added

### `core/src/events.rs` — bounded event log

- `NodeEvent` enum with labeled variants (node started, peer
  discovered/lost, transfers, repairs, health changes, custom)
- `EventLog` with a hard capacity bound (default 512), tick-based recording,
  `since(tick)` filtering and label counting
- JSON serializable; oldest entries are dropped automatically

### `core/src/metrics.rs` — metrics registry

- Counters (`incr`, `add`), gauges and bounded latency samples
  (max 64 per metric, most recent kept)
- `mean` and nearest-rank `p95` helpers plus a merged `summary()` snapshot
- Deterministic: no wall clock, no threads, no allocation on read

### `core/src/search.rs` — local search index

- Case-insensitive alphanumeric tokenizer
- Inverted index (term → document → frequency) with tf scoring
- Re-indexing replaces previous content; removal cleans the postings
- `query(text, limit)` returns ranked (doc id, score) pairs

### `core/src/scheduler.rs` — maintenance scheduler

- `Scheduler` with `add_job(name, interval_ticks)`, validation and typed
  errors via `SchedulerError`
- `advance()` returns the jobs due on each tick, in registration order
- `ticks_until_next()` for dashboards; fully deterministic and replayable

## Integration notes

- Modules are exported from `core/src/lib.rs`.
- No new dependencies: `serde`, `serde_json` and `thiserror` only, all
  already in the workspace manifest.
- CI (`core.yml`) already runs `cargo fmt --check`, `cargo test` and
  `clippy -D warnings` over the new modules.

## Status rule

As with the rest of the workspace: source plus tests is not proof of
end-to-end functionality. These subsystems are local and deterministic, so
they are validated by unit tests in CI; their effect on distributed
behaviour must still be observed on independent nodes per
[REAL-NETWORK-TEST.md](REAL-NETWORK-TEST.md).
