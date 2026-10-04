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
acceptance model.

## Security

Authenticated transport and cryptographic primitives protect different
threats. They do not by themselves provide anonymity. Never log secrets,
private keys or plaintext private content.

## License

MIT

## Try the real product

The desktop build launches the bundled AWE node and opens the local dashboard. For a real distributed Drive test, run at least three reachable AWE nodes, connect them from **Peers & Connections**, upload a file from **Storage**, then reconstruct it using the returned file ID. Drive data is encrypted on the owner before sharding and remote replication.

For Messenger, connect two independent nodes, use each node's AWE ID in the **Messenger** screen, send a message, and verify the remote delivery acknowledgement.

Real Internet/NAT diversity and clean-machine release acceptance require environment-specific validation; the source tree alone does not claim those gates are complete.
