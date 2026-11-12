# Marketplace catalog sync upgrade (2026-11-12)

## Summary

This upgrade gives the AWENET Store its network-wide catalog. Until now the
storefront was an island: `storefront.rs` owned a node's own catalog,
`store_escrow.rs` settled its orders on the local ONECOIN ledger, and a buyer
could only search products listed on the node they were connected to. A new
core module (`marketplace_sync.rs`, stream 203 on the authenticated node
transport) is the wire between seller nodes and their peers, and `awe-node`
wires it end to end: signed listing and delisting announcements, a bounded
last-writer-wins remote catalog per node, immediate announcement after a
storefront change, and a network-wide search endpoint.

## Motivation

The economy product layer kept one last gap open since the 2026-11-06
upgrade: the store was single-node. The verifier transport (2026-11-11)
closed the usage-verification gap; this upgrade closes the catalog half of
the marketplace gap. Sellers get network-wide discovery for their storefront
without giving up any economy rule: the catalog carries no money and no
authority.

## Changes

- **core/src/marketplace_sync.rs (new)**: `ListingAnnounceCore` and the
  signed `ListingAnnounce` on stream 203, JSON-encoded with a 64 KiB bound;
  `RemoteCatalog`, the bounded receiving side (per-seller bound of 128
  listings, total bound of 2,048, delisting tombstones, 6-hour retention
  prune); `storefront_announces`, the per-cycle announcement builder
  (own products only, active listings first, delistings behind them, capped
  at the per-seller bound so sender and receiver agree). Acceptance rules:
  the announcing peer must be the seller itself, the announcement must be
  well formed (storefront text bounds, positive price and stock, socket
  seller address), the signature must verify under the public key the
  seller AWE-ID was derived from, and only a strictly newer revision changes
  anything.
- **core/src/policy.rs**: stream 203 joins the allowed streams.
- **apps/awe-node/src/main.rs**: the dispatcher handles stream 203 and
  merges accepted announcements into the node's remote catalog; a 15 s
  `marketplace_sync_cycle` announces this node's own storefront to every
  connected peer and prunes stale remote listings; a fresh listing, update
  or delisting announced over the node API is announced to peers right
  away; the UI address is parsed once and published as the seller node
  address.
- **apps/awe-node/src/economy_api.rs**: `GET /api/storefront/network`
  (network catalog with the established filter surface: `q`, `kind`,
  `category`, `seller`, `min_price_atoms`, `max_price_atoms`), with an
  honest notice that a listing points at the seller's node and is not an
  order route.
- **apps/awe-node/tests/product_smoke.rs**: a live three-node section —
  node 2 lists a product, node 1 must discover it in the network catalog
  with node 2's seller node address, node 2 delists it, node 1's catalog
  must drop it.
- **README.md**: layer-table row, Marketplace catalog sync section.

## Deployment note

The default `NetworkPolicy` ships with stream 203 allowed. A node that
already persisted an `awenet-policy.json` before this upgrade keeps its
saved `allowed_streams` and will silently drop stream 203 traffic until the
operator adds `203` to the file's `allowed_streams` list; the policy is
re-read from disk while the node runs, so no restart is needed.

## Economy rules, unchanged and enforced

- Nothing here mints ONECOIN. Store revenue stays a buyer-to-seller
  transfer on the seller's ledger; the network catalog is discovery data.
- A seller sets its own product prices in ONECOIN atoms; nobody but the
  founder changes the official ONECOIN price, and this transport touches
  no prices at all.
- A peer cannot forge listings: the sender must be the seller, the
  signature must verify, and the AWE-ID must derive from the announced
  public key.
- A delisted product cannot be resurrected by a reordered stale
  announcement: the tombstone holds the newer revision.
- A silent seller disappears from the peers' catalogs after the
  retention horizon instead of lingering forever.

## Verification

- 10 unit tests in `marketplace_sync.rs` (roundtrip/malformed/oversized
  rejection, well-formedness, forged/foreign/malformed rejection,
  last-writer-wins merge, delisting tombstones, per-seller and total
  bounds, retention prune, filter surface over remote listings,
  storefront announce building and bounding, full announce-to-catalog
  flow).
- 1 economy_api test (network catalog endpoint: discovery data, seller
  node address, honest notice, filter surface, invalid seller filter).
- The live three-node smoke section runs real node processes and asserts
  cross-node discovery and delisting over stream 203.
- CI runs all of it on Ubuntu and Windows.

## Honestly not implemented

- Cross-node order execution is deliberately not part of this transport:
  escrow funding is local-ledger, so a buyer's node funding an order on
  the seller's node needs an order handshake and a funding protocol of
  its own. A network listing shows where the seller node accepts buyers;
  the purchase itself still happens on the seller's node.
- The remote catalog lives in memory only: sellers re-announce every
  cycle, so a restart simply starts collecting again, and a restarted
  node loses its remote view until the next announce cycle (at most 15 s
  while the sellers are online).
- Announcements travel one hop, to connected peers only: there is no
  relay or flood, so a listing is only visible where the seller is a
  direct peer.
- A seller with more products than the per-cycle bound (128, active
  listings first) syncs only the first slice of its catalog; products
  beyond it disappear from peers only through the retention prune.
- `sold_units` is unknown remotely and honestly reported as zero in the
  network catalog.
