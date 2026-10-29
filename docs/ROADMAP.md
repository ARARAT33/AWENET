# AWENET product roadmap

This roadmap reflects the release acceptance model in
[PRODUCT-ARCHITECTURE.md](PRODUCT-ARCHITECTURE.md) and the readiness gates in
[PRODUCT-READINESS-GATES.md](PRODUCT-READINESS-GATES.md). A phase is done only
when it is proven end-to-end on independent nodes, not when the code exists.

| Phase | Scope | Status |
| --- | --- | --- |
| 1. Sovereign core | Identity vault, authenticated encrypted transport, hostile-traffic defense | Done — maintained in CI |
| 2. Data plane | Content-addressed storage, replication, repair, recovery | Done — maintained in CI |
| 3. Product orchestration | `product.rs` facade, health score, readiness gates | Done — health score live |
| 4. Desktop product | Embedded UI, Messenger, calls, groups & channels, Browser, AWEStore | In progress — themes, command palette and live HUD shipped |
| 5. AWENET federation | `.awenode` / `.awedc` / `.dgc` memberships across real nodes | In progress — generation & import shipped, real-network evidence pending |
| 6. ONECOIN economy | Ledger, wallet transfers, exchange, contribution tiers | In progress — consensus runtime shipped, multi-node validation pending |
| 7. Platform reach | Linux daemon, Windows service, Android, AWEOS | Partial — platform boundaries enforced in CI |
| 8. Release acceptance | Clean-machine release tests, independent-node evidence, published binaries | Open — tracked in REAL-NETWORK-TEST.md |

## Next up

- Federation UX tour with two live nodes and recorded evidence
- ONECOIN consensus run on three independent nodes with published transcripts
- Release binaries for Linux, macOS and Windows with clean-machine verification

## Shipped 2026-10-29 — observability & automation layer

- `events.rs`: bounded, replayable node event log (audit + observability) with
  labeled events, tick filtering and JSON serialization
- `metrics.rs`: in-process metrics registry — counters, gauges and bounded
  latency samples with mean and p95, feeding the product health pipeline
- `search.rs`: local inverted index with term-frequency scoring for the
  Browser and Store screens, with re-index and removal support
- `scheduler.rs`: deterministic tick-driven maintenance scheduler for repair
  sweeps, backups and health recomputation — replay-friendly, fully tested
- All four modules use only std and existing dependencies, with unit tests
  maintained in CI (fmt, test, clippy -D warnings)

