# Commerce & economy services upgrade (2026-11-03)

This upgrade extends the AWENET economy with product commerce primitives as
four deterministic core modules. Like every AWENET service module they are
pure data: no wall clock, no threads and no ambient I/O. Time is an
explicit tick supplied by the caller, which keeps behaviour replay-friendly
and testable. Amounts are unsigned minor units; there are no floats.

## New modules

| Module | Purpose |
| --- | --- |
| `core/src/accounting.rs` | Double-entry ledger with balanced journal entries |
| `core/src/billing.rs` | Invoice lifecycle with line items, payments and overdue sweeps |
| `core/src/subscriptions.rs` | Subscription lifecycle with trial, renewal, grace and expiry |
| `core/src/escrow.rs` | Escrow with fees, release, refund and expiry |

## accounting.rs

`Ledger` keeps accounts classified as Asset, Liability, Equity, Revenue or
Expense. `post` validates every journal entry: a non-empty name, 2..=100
lines, exactly one nonzero side per line, known accounts, and total debits
equal to total credits. Unbalanced entries are rejected with a typed error
carrying both sides. Balances are signed minor units (debits minus credits)
and `trial_balance` lists every account in order.

## billing.rs

`Billing` creates open invoices from priced line items (label, quantity,
unit price) with a due tick. `total` sums line totals with checked
arithmetic, `record_payment` allocates payments against the remaining
amount and closes the invoice as Paid only when fully covered —
overpayment and zero payments are typed errors. `void` cancels open
invoices, `overdue` sweeps open invoices past due, and `stats` summarises
open, overdue, paid and void counts.

## subscriptions.rs

`Subscriptions` manages plans (period, grace and price) with the states
Trialing, Active, PastDue, Cancelled and Expired. `subscribe` optionally
starts a trial window, `activate` converts a trial into a full active
period, `renew` extends trialing or active subscriptions by one period,
`cancel` stops the clock, and `advance` applies the deterministic
lifecycle transitions at a tick: lapsed trials expire, ended periods move
to PastDue, and exhausted grace expires the subscription.

## escrow.rs

`Escrows` holds an amount between two distinct parties with an optional
fee and expiry tick. `fund` locks the amount (expired escrows refuse
funding), `release` pays the seller the amount minus the fee, `refund`
returns the full amount to the buyer, and `expire` sweeps unfunded and
held escrows whose expiry passed.

## Verification

- `cargo test --workspace` — the four modules ship with unit tests covering
  validation, lifecycles, balancing, payment allocation, grace and expiry.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` — the modules follow the repository style and lint set.

The economic primitives fail closed on arithmetic overflow: journal balances,
invoice totals, lifecycle ticks, escrow terms and simulator delivery ticks are
checked before state is committed. Invalid empty identities and backwards due
or expiry windows are rejected, and failed mutations leave the prior state
unchanged.

## Layer summary

The README layer table now includes a Commerce & economy services row,
joining product orchestration, observability, platform, smart, trust &
lifecycle, intelligence & resilience and developer & API services.
