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