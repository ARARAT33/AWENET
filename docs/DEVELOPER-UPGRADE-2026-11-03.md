# Developer & API services upgrade (2026-11-03)

This upgrade adds a developer and automation plane to AWENET as four
deterministic core modules. Like every AWENET service module they are pure
data: no wall clock, no threads and no ambient I/O. Time is an explicit tick
supplied by the caller, which keeps behaviour replay-friendly and testable.

## New modules

| Module | Purpose |
| --- | --- |
| `core/src/tasks.rs` | Bounded priority task queue with deadlines and retry budgets |
| `core/src/keys.rs` | Scoped API key lifecycle: issue, verify, rotate, revoke, expiry |
| `core/src/webhooks.rs` | Event subscriptions with cooldowns, retry hints and dead-lettering |
| `core/src/runbooks.rs` | Gated runbooks with dry-run plans and abort-with-reason execution |

## tasks.rs

`TaskQueue` accepts up to `MAX_TASKS` tasks. Every task carries a priority
(Low / Normal / High / Critical), an optional deadline tick and a retry
budget. `next_ready` picks the highest-priority unexpired pending task,
`start` consumes one attempt, `fail` requeues within the budget or dead-ends
the task as Failed, `expire` fails every overdue pending task and `stats`
summarises the queue by state.

## keys.rs

`ApiKeys` issues deterministic `awenet-<id>-<token>` key material from an
internal counter (splitmix64), with a label, scope set and optional expiry
tick. `verify` returns typed errors for unknown, revoked and expired keys,
`rotate` revokes the old material and issues fresh material with the same
label, scopes and expiry, and `active` lists the keys that verify at a tick.

## webhooks.rs

`Webhooks` registers up to `MAX_WEBHOOKS` hooks, each listening on event
kinds with its own attempt budget and cooldown. `plan` renders per-hook
delivery plans for an event: a ready attempt number, a cooldown deferral with
`retry_after_ticks`, or a dead letter once the budget is exhausted.
`record_success` clears the failure streak; `record_failure` advances it.

## runbooks.rs

`Runbooks` stores ordered runbooks whose steps declare gates: a minimum
product-health score and a maximum open-alert count. `plan` renders a
dry-run per step with concrete skip reasons; `execute` applies every action
only when all gates pass and aborts at the first failing gate with a reason.

## Verification

- `cargo test --workspace` — the four modules ship with unit tests covering
  validation, lifecycles, budgets, cooldowns and gate behaviour.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` — the modules follow the repository style and lint set.

## Layer summary

The README layer table now includes a Developer & API services row, joining
product orchestration, observability, platform, smart, trust & lifecycle and
intelligence & resilience services.
