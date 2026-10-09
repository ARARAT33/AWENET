# AWEp2P

AWEp2P is the decentralized networking and resource layer of the AWE ecosystem.
It keeps the full architecture—identity, secure transport, discovery, routing,
distributed storage, repair, names/sites, Store, Messenger and desktop clients—
while exposing a small product orchestration model.

## Product architecture

The product facade is in core/src/product.rs. It gives the existing subsystems
one stable model for resources, peers, placement, transfers, capacity and
product health. It does not replace the deeper protocol modules.

The intended runtime is:

Desktop / CLI
  -> Product orchestration
  -> identity + security
  -> discovery + routing
  -> authenticated transport
  -> storage + replication + repair
  -> names/sites + Store + Messenger
  -> other AWE nodes

## Important status rule

A source module or UI screen is not proof of end-to-end functionality.
Production readiness requires evidence from independent nodes and clean-machine
release tests.

## Build and test

    cargo fmt --all -- --check
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings

For distributed validation, use docs/REAL-NETWORK-TEST.md.

See docs/PRODUCT-ARCHITECTURE.md for the complete architecture and release
acceptance model. Use [docs/PRODUCTION-READINESS-GATES.md](docs/PRODUCTION-READINESS-GATES.md)
as the prioritized release checklist; gates remain unverified until test evidence
is recorded.

## Security

Authenticated transport and cryptographic primitives protect different
threats. They do not by themselves provide anonymity. Never log secrets,
private keys or plaintext private content.

## License

MIT
