# Trust & lifecycle services upgrade — 2026-11-01

This upgrade adds a fifth product-grade layer to AWENET: **trust &
lifecycle services**. It follows the same rules as the observability (PR
#67), node platform (PR #74) and smart services (PR #77) layers —
deterministic, bounded, replay-friendly modules with no wall clock and no
threads, validated by unit tests and the standard CI gates.

## New modules

| Module | What it gives you |
| --- | --- |
| `core/src/sessions.rs` | Bounded authenticated session manager: explicit-tick TTL, typed Active/Expired/Revoked/Unknown states, immediate revocation and auditable sweeps |
| `core/src/consent.rs` | Bounded consent ledger: per-(subject, scope) grants with expiry ticks, revocation, expiry sweeps and an ordered audit log |
| `core/src/features.rs` | Deterministic feature flags: per-flag kill switch and rollout percent, stable FNV-1a subject bucketing so every node evaluates identically |
| `core/src/sync.rs` | Deterministic last-writer-wins sync: version + writer-id conflict rule that converges from any apply order, with a `diff()` for delta exchange |

## Design rules kept

- **Deterministic**: time is an explicit tick counter supplied by the
  caller; rollout bucketing and conflict resolution use stable hashes and
  ids, never randomness or wall clocks.
- **Bounded**: every structure has an explicit maximum (`MAX_SESSIONS =
  4,096`, `MAX_GRANTS = 1,024`, `MAX_LOG_ENTRIES = 4,096`,
  `MAX_FLAGS = 256`, `MAX_KEYS = 65,536`) and returns typed errors
  instead of growing without limit.
- **Serializable**: managers and stores serialize to JSON, so they can be
  persisted, inspected and replayed.
- **First-class errors**: every module has its own `thiserror` error enum.
- **Tested**: every module ships unit tests covering validation, the main
  behaviour paths, edge cases and round-trip serialization.

## How the pieces fit

- `sessions` is the natural companion of `identity`: identity proves who
  a principal is once, sessions carry that decision across ticks with
  explicit expiry and revocation.
- `consent` feeds the same policy plane as `policy` and `permissions`:
  `is_allowed(subject, scope, now_tick)` is the single check shared-data
  access paths should call before reading or exporting.
- `features` pairs with `update`: `update` governs node versions, while
  `features` gates individual product capabilities with safe, reversible
  rollouts and a kill switch.
- `sync` complements `replication`: replication handles chunk durability
  for content-addressed data, while `sync` gives smaller replicated
  state (settings, quotas, consent mirrors) a convergence guarantee
  without a coordinator.

## Verification

- `cargo fmt --all -- --check`
- `cargo test --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- No protocol or persistence format changed; all additions are new, purely
  additive modules registered in `core/src/lib.rs`.
