# Developer & API services upgrade (2026-11-03)

This upgrade adds a bounded developer and automation plane to AWENET. Lifecycle
decisions use explicit caller-supplied ticks and remain deterministic, while
bearer credentials are deliberately generated from the operating system's
cryptographically secure random source. The registry stores only one-way
fingerprints, never reusable API-key secrets.

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

`ApiKeys` issues 256-bit bearer secrets using the OS CSPRNG. The secret is
returned once in `IssuedApiKey`; its custom `Debug` implementation redacts the
secret, and the registry retains only a domain-separated SHA-256 fingerprint.
Labels, scopes and expiry are validated, scopes are normalized, and malformed
or unknown credentials produce errors that never echo the supplied secret.
Rotation creates the replacement before revoking the old credential, so an
entropy or capacity failure leaves the old key valid. The key registry is a
core primitive only: it is not yet persisted or connected to HTTP route
authorization, and no API endpoint should be considered protected by it.

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
