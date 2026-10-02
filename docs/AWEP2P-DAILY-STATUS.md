# AWEP2P Daily Engineering Status — 2026-10-02

## Architecture consolidation

This pass keeps the original large AWEP2P architecture but makes its product
boundary explicit. The existing identity, security, protocol, discovery,
routing, storage, replication, repair, namespace, host, store, messenger and
platform modules remain the implementation layers.

A new small product facade, core/src/product.rs, now provides shared types for
resource identity, manifests, capacity, peers, transfers, placement policy and
product snapshots. This is intended to prevent the end-user product from
becoming a collection of disconnected subsystem APIs.

## What is implemented

- Rust workspace and AWE node executable.
- Identity and cryptographic primitives.
- Authenticated transport and replay defenses.
- Peer, discovery and routing primitives.
- Content-addressed storage and replication/repair models.
- Namespace, host/site, Store and Messenger foundations.
- Desktop and platform scaffolding.
- CI workflows.
- Unified product orchestration model with unit tests.

## What still needs end-to-end proof

The following remain release gates rather than claims of completion:

1. Two independently launched nodes connect and reconnect.
2. Discovery finds usable peers without manual configuration.
3. A resource uploads over the real authenticated data path.
4. A remote node durably verifies and stores the received bytes.
5. A resource downloads and reconstructs correctly after interruption.
6. Replica loss is detected and repaired with real bytes.
7. Namespace/site content resolves and retrieves remotely.
8. Store packages are verified and capability boundaries are enforced.
9. Messenger delivery works across independent nodes.
10. Desktop launch, onboarding and clean-machine installation work without a terminal.

## Validation rule

The project is not marked production-ready merely because the source tree has
all architectural modules. The release percentage must move to 100 only after
the acceptance evidence above is actually passing.
