# Developer & API services upgrade (2026-11-03)

This upgrade adds a developer and automation plane to AWENET as four
bounded core modules. Task, webhook, runbook and key-lifecycle decisions use
explicit ticks and remain replay-friendly. API-key material is intentionally
non-deterministic and comes from the operating-system CSPRNG; the registry
retains only fingerprints. Key issuance fails closed if secure entropy is
unavailable.

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

`ApiKeys` issues 256-bit bearer secrets from the operating-system CSPRNG
using the `awenet_<hex>` format. The registry stores only a domain-separated
SHA-256 fingerprint, never the raw secret. The `IssuedApiKey.secret` value is
returned only on issue/rotate; its Debug output is redacted, and typed errors
never echo a supplied credential. If OS entropy is unavailable,
issuance fails closed with EntropyUnavailable; there is no deterministic
fallback. `verify` checks unknown, revoked and expired keys. `rotate` registers the replacement before revoking the old key,
so a capacity or entropy failure leaves the old credential valid. Lifecycle
and expiry decisions still use explicit ticks. The module is a core primitive;
it is not yet wired into the HTTP route authorization layer.

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
