# AWEP2P / AWE Net — Production Readiness Roadmap

## Implemented foundations
- Node-to-node topology state.
- Data-centre full mesh and relay topology primitives.
- Data Group / Centre Group hierarchy validation.
- Peer routing primitives.
- Authenticated identities and encrypted vaults.
- Encrypted fixed-size A2P2 packet primitive.
- TCP encrypted session layer with admission controls, replay protection, heartbeats and peer discovery primitives.
- Distributed storage placement and replica health primitives.
- Host/site manifest and content-integrity primitives.
- CI architecture-integrity checks.

## Remaining production layers
1. Real-world transport hardening: multi-transport support, NAT traversal and IPv4/IPv6 behavior.
2. Automatic discovery with redundant signed peer records.
3. Automatic topology convergence for nodes, Data Centres, Data Groups and Centre Groups.
4. Heartbeat-driven failure recovery, reconnect backoff, alternate routes and relay failover.
5. Scalable indexed routing with bounded lookup/fanout.
6. Live data-plane integration for application, storage and hosting traffic.
7. Automatic replica repair after node failure.
8. Live AWEwww/AWETLD/AWEOpen resolution over the P2P network.
9. Real peer-session Messenger integration.
10. Operational metrics, health reporting and diagnostics.
11. Adversarial multi-node, partition/rejoin, malformed-packet and flood testing.
12. Reproducible deployment, upgrades and release verification.

## 100% definition
AWE Net is 100% operational only when a fresh node can discover peers, authenticate, establish encrypted transport, join the nearest healthy topology path, participate in its Data Centre, reach other Data Centres through full-mesh or redundant relay paths, route application data, store/retrieve replicated data, recover from node loss, and pass Linux + Windows integration CI.

The repository currently contains substantial foundations, but these primitives and CI results are not proof that a globally deployed AWE Net already exists.
