# AWE Net — Product Roadmap and Readiness

## Product definition
AWE Net is the intended P2P substrate for AWE nodes and applications. Topology and routing models do not by themselves mean the network is operational.

## Foundations in the repository
The codebase includes identity/vault and cryptographic primitives, authenticated encrypted TCP session functionality, peer and routing primitives, topology modeling, storage placement and replica-health structures, namespace/manifest structures, and platform/client scaffolding. Verify current code and tests before making release claims.

## Remaining product tracks
1. **Node operations:** first run, lifecycle, persistence, resource controls, shutdown, and recovery.
2. **Connectivity:** independent-node connections, reconnect/backoff, timeouts, IPv4/IPv6, and observed peer health.
3. **Discovery/routing:** redundant discovery, persistent peer records, bounded lookup, and tested multi-hop forwarding.
4. **Storage data plane:** remote encrypted shard upload/download, durable acknowledgements, integrity, resumability, and reconstruction.
5. **Self-healing:** health-driven replica repair with actual byte movement/reconstruction and safe metadata commits.
6. **Names/sites:** signed/fresh records and remote content retrieval.
7. **Applications:** verified distribution, capability enforcement, and real sandbox execution where advertised.
8. **Messenger:** reliable remote delivery; encrypted real-time media for calls.
9. **Clients:** terminal-free UI, onboarding, accessibility, diagnostics, and consistent core behavior.
10. **Release operations:** reproducible builds, signing, clean installs, upgrades, rollback, backup, and support documentation.

## Operational acceptance
A release must demonstrate a fresh node joining without a central account, authenticating peers, discovering a usable route, exchanging application data, persisting and retrieving remote content, recovering from peer loss, and installing/upgrading on each declared supported platform. Evidence must be repeatable and tied to the exact source revision.

## Current classification
**Experimental foundation; production readiness not established.** The last documented estimate is ~33% (2026-09-30), a rough engineering estimate—not a verified metric. Recalculate only after a fresh audit with a defined denominator and evidence.