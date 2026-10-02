# AWEp2P

AWEp2P is an open, privacy-oriented peer-to-peer networking project intended to provide node infrastructure for AWEwww/AWETLD, storage, hosting, applications, and messaging.

## Current status

**Maturity: foundation / experimental. Not a production-ready end-user product.** The repository contains a Rust core, node application, desktop UI, platform integration boundaries, and protocol primitives. The presence of a module, UI screen, data structure, or packaged binary does not by itself mean the corresponding service works end to end.

The latest documented engineering estimate is approximately 33% overall maturity (2026-09-30). This is a rough engineering estimate, not a measured completion metric. It should not be treated as a current test result.

## Intended products

- **AWE Node:** identity, peer connections, resource controls, diagnostics, and node lifecycle.
- **AWE Drive:** private file storage, encryption, chunking, replication, retrieval, and recovery.
- **AWE Host / AWE Sites:** publish and retrieve content through nodes and names.
- **AWE Store:** verify and distribute applications and their permissions.
- **AWE Messenger:** private messaging and, later, voice/video.
- **AWEwww / AWETLD / AWEOpen:** decentralized names, registry, resolution, and browsing.
- **Desktop and mobile clients:** user-facing control of the shared node/core.

These are product goals; each feature's implementation status must be checked against the code and the readiness gates.

## Architecture

The Rust core is shared across applications. The node process owns long-lived network activity; clients should use a well-defined local interface. Protocol messages must be versioned and authenticated at trust boundaries. Private content should be encrypted before leaving its owner's device.

## Current implemented foundations

The repository includes identity/vault and cryptographic primitives, authenticated TCP session functionality, replay defenses, peer and routing primitives, content-addressed local storage, erasure/replication planning, namespace and manifest structures, and desktop/platform scaffolding. Exact boundaries and remaining gaps are recorded in `docs/AWEP2P-DAILY-STATUS.md`.

## Important limitations

Do not assume that the following are production-complete without current end-to-end evidence: persistent DHT, Internet-scale discovery, multi-hop forwarding, NAT traversal, remote shard upload/download, automatic repair, site retrieval from remote nodes, sandboxed application execution, real-time media, signed installers, updates, or recovery on clean machines.

Privacy protections reduce exposure but do not guarantee anonymity. IP addresses, timing, traffic volume, relays, endpoints, and operating systems can expose metadata.

## Build and test

Use the Rust workspace commands documented by the repository and CI. For a real two-node experiment, follow [docs/REAL-NETWORK-TEST.md](docs/REAL-NETWORK-TEST.md). A successful local build is not proof of network interoperability.

## Project documentation

- [Engineering charter](docs/AWEP2P-ENGINEERING-CHARTER.md)
- [Daily engineering status](docs/AWEP2P-DAILY-STATUS.md)
- [Product readiness gates](docs/PRODUCT-READINESS-GATES.md)
- [Real node-to-node test](docs/REAL-NETWORK-TEST.md)
- [AWE Net roadmap](docs/awe-net-production-readiness.md)
- [Network topology model](docs/network-topology.md)

## License

See [LICENSE](LICENSE).
