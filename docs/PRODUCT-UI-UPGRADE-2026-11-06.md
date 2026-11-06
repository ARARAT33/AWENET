# Economy Product Layer — 2026-11-06

This upgrade turns the existing deterministic ONECOIN economy core into a
working product layer inside the local node: a storefront catalog module in
the core, an economy HTTP API in `awe-node`, and four UI screens
(AWENETwork board, ONECOIN Market, Founder Control Center, wallet receive
QR). Everything persists to disk, every payout still goes through the
founder-approved, idempotent mint, and store revenue always moves
buyer → escrow → seller instead of minting.

## What was added

### `core/src/storefront.rs` (new core module)

- `ProductKind` (digital / service / physical), `Product`, `ProductFilter`
  (query, kind, category, seller, min/max price, include-inactive).
- `Storefront`: `list_product`, `update_product`, `deactivate_product`,
  `search`, `seller_products`, `seller_stats`, `record_sale`.
- Bound checks: `MAX_PRODUCTS = 10_000`, title 128 chars, description 2 000
  chars, category 64 chars, positive price, no zero-stock listings.
- Sellers may only update their own listings; self-purchase is rejected.
- Unit tests, including bounds.

### `apps/awe-node/src/economy_api.rs` (new node API)

Persistent `NodeEconomy` state (`awenet-economy.json`) holding the founder
authority, official price control, treasury, AWENET work market, storefront
and store escrow, plus a dedicated marketplace operator account (buyer,
seller and operator are always three distinct accounts).

Endpoints served by the node UI server on `127.0.0.1:41800`:

| Area | Endpoint | Notes |
| --- | --- | --- |
| Public reads | `GET /api/onecoin/price` | Official price, history, disclaimer |
| | `GET /api/awenetwork/tasks`, `GET /api/awenetwork/task/{id}` | Filters: category, complexity, status, min reward |
| | `GET /api/awenetwork/profile` | Contributor profile, level, own submissions |
| Participant actions | `POST /api/awenetwork/task/{id}/accept` | Accept a published task |
| | `POST /api/awenetwork/task/{id}/submit` | Submit evidence for review |
| | `POST /api/awenetwork/submission/{id}/resubmit` | After a founder revision request |
| | `POST /api/awenetwork/suggest` | Community suggestion, never official until approved |
| Founder-only | `GET /api/founder/identity`, `GET /api/founder/overview` | Founder gate + economy dashboard |
| | `POST /api/awenetwork/tasks` | Publish an official task (founder key required) |
| | `POST /api/founder/submissions/{id}/approve` / `approve-team` / `reject` / `revise` | Personal review; approve mints idempotently |
| | `POST /api/founder/price` | Official price change with reason and history |
| | `POST /api/founder/tasks/{id}/close` / `cancel` | Task lifecycle |
| | `POST /api/founder/suggestions/{id}/approve` | Suggestion becomes an official task |
| | `POST /api/founder/store/orders/{id}/resolve` / `refund` | Dispute ruling, founder refund |
| Storefront | `GET /api/storefront/products` | Search + filters |
| | `GET /api/storefront/seller`, `GET /api/storefront/orders` | Seller page, own orders with escrow status |
| | `POST /api/storefront/products` / `{id}/update` / `{id}/deactivate` | Listing management |
| | `POST /api/storefront/orders` | Open + fund an escrowed order |
| | `POST /api/storefront/orders/{id}/confirm` / `dispute` | Release escrow or open a dispute |

Guarantees kept by the API layer:

- Founder-only routes are gated by `FounderAuthority::is_founder` in the
  backend, not by hiding buttons.
- Approvals pay through `WorkMarket::approve_and_pay` →
  `OnecoinLedger::mint_approved_reward` (verifier-gated, idempotent by
  reward id). The node registers itself as a reward verifier at startup.
- Treasury records every flow (genesis/join issuance, task rewards, store
  purchases, escrow hold/release/refund, seller income, marketplace fees)
  and `sync_treasury_issuance` keeps treasury issuance equal to the real
  ledger total, so `treasury.reconcile` never reports fake reserves.
- A background sweep expires held orders every 60 seconds and refunds the
  buyer through escrow, recording `EscrowRefund` in the treasury.
- Lock order is always economy-state lock → ledger lock, so concurrent
  handlers cannot deadlock.

### Core module extensions (no behaviour changes)

- `escrow.rs`, `store_escrow.rs`: serde derives for full-economy
  persistence; `StoreEscrow::orders()` iterator; round-trip tests.
- `awenetwork.rs`: `submissions()` / `suggestions()` iterators for the
  review queues; round-trip test.

### UI screens (`apps/awe-desktop/ui/`)

- `awenetwork.js` + `economy.css`: AWENETwork task board with filters,
  task detail, accept/submit/resubmit, suggestions, personal work profile.
- `storefront.js`: ONECOIN Market — catalog search, product cards, buy
  with ONECOIN (escrow), seller page (list/update/deactivate), order list
  with confirm-delivery and dispute actions.
- `founder.js`: Founder Control Center — economy overview, task publishing
  (phases, team flag), submission review (approve / approve-team with split
  / reject / revise), official price control with history, suggestion
  approval, store order dispute ruling and refunds. The nav item is hidden
  unless `GET /api/founder/identity` reports the founder.
- `onecoin.js` bug fix: the wallet tab previously crashed on bindings to
  non-existent `ocReceiveQr`/`ocScan` elements, silently disabling Send and
  the exchange/rewards forms. Receive is now a real QR modal
  (`vendor/qrcode.js`); the camera scan (never implemented) was removed.
- `index.html`: new nav entries and script tags; served by `main.rs`
  (`/awenetwork.js`, `/storefront.js`, `/founder.js`, `/economy.css`).

## Explicitly NOT implemented (honest status)

The following were listed as remaining work in
[ECONOMY-COMPLETION-2026-11-05.md](ECONOMY-COMPLETION-2026-11-05.md) and are
still not implemented. Nothing in this PR fakes them:

1. **Browser WebAuthn ceremony** — `founder_guard.rs` provides passkey
   enrollment and step-up challenges in the core, but the browser
   ceremony (navigator.credentials) is not wired into this UI.
2. **Multi-node reward verifiers** — payouts mint on the local node with
   the node as sole registered verifier; deploying the multi-verifier
   settlement of `resource_meter.rs` across real nodes is future work.
3. **GPU / storage metering** — `resource_meter.rs` meters CPU/RAM from
   /proc on Linux; GPU and storage usage reports are not implemented.
4. **Multi-node marketplace sync** — the storefront, escrow and task board
   are local-node state; replicating products/orders/tasks between peers is
   future work.
5. **UI localization** — the economy screens are English, matching the rest
   of the desktop UI; the `i18n.rs` bilingual strings are not wired in.
6. **Camera QR scanning** — receiving shows a QR of your AWE ID; scanning a
   QR with the camera is not implemented.

## Testing

- New unit tests: `storefront.rs` (6), escrow/store_escrow/awenetwork
  round-trip and iterator tests.
- Existing workspace CI (fmt, tests, clippy on Linux and Windows,
  architecture integrity, audit) applies to this layer; the economy API is
  part of the `awe-node` binary built and checked by the same workflows.
