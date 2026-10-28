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
