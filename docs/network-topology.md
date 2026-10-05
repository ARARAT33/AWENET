# AWEP2P Network Topology Model

## Conceptual hierarchy
`Node → Data Centre → Data Group → Centre Group → AWE Net`

This is a logical organization model, not a guarantee that every node is connected or that topology converges automatically.

## Nodes and peers
A node may maintain connections to multiple peers. The connected-peer set is runtime state, not an intended-peer list. Entry-peer selection must use an explicit measurable policy rather than fabricated geographic assumptions.

## Data Centres and inter-centre links
A Data Centre groups nodes for policy and administration. Full mesh is a possible configuration, not an unconditional property. Inter-centre links may be direct or relay-mediated; a modeled edge is operational only when authenticated sessions are live and forwarding works.

## Groups
Data Groups and Centre Groups organize multiple Data Centres/groups. Membership constraints do not create network connections.

## Routing and resilience
Routing must use current verified peer state. A route is successful only when each hop forwards a correlated request within a bounded hop count and returns its response. Failover requires a tested alternate path and recovery behavior.

## Production evidence
Test direct sessions, multi-hop forwarding, node loss, partition/rejoin, stale-peer removal, and relay failover with independent processes. Until then, this document describes a topology model—not a live global network.

## Scalable discovery architecture

Bootstrap is a multi-source control-plane operation, not a permanent dependency. Nodes query several seeds concurrently, learn signed peer records, perform bounded iterative lookups, and periodically refresh from both configured seeds and learned peers. The routing cache is bounded so a node cannot grow memory linearly with network size.

The target architecture is therefore **many small local routing views**, not a global routing table replicated to every device. A node only needs enough nearby/close peers to reach the next lookup frontier.

## Access architecture

Every addressable resource has an explicit visibility mode:

- **Open:** the resource ID is sufficient to request it.
- **Secret:** the resource publishes only a one-way access-map commitment; possession of the access map is required to authorize access.
- The access map is a capability and must be treated like a private key. It is never required to be published to the network.

Secret access is an application/data-plane authorization boundary. Encrypted transport alone is not treated as authorization.

## Global-network reality

No protocol can guarantee that literally every Internet device joins a P2P network: NATs, firewalls, carrier networks, offline devices, OS background limits, and missing Internet reachability still exist. AWEP2P should therefore maximize reachability through multi-source discovery, IPv4/IPv6, direct connections, and real relay/NAT traversal implementations, while never pretending a modeled route is a live route.
