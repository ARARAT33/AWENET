# Network topology invariants — 2026-11-09

## Scope

This document records the invariants enforced by `core/src/network_topology.rs`.
The module is the in-memory topology model used by the Phase 2 network-control
work. It is not yet the node's persistent, signed network-control API.

## Membership rules

- Data-centre IDs must be non-empty and unique.
- Node IDs must be non-empty and globally unique across data centres.
- A node cannot be inserted twice into one data centre.
- A newly inserted live node with no explicit peers is attached to the nearest
  live node in both directions. A caller-supplied peer must already belong to
  the same data centre.
- Data-group IDs must be non-empty and unique. A data group must contain at
  least three distinct, existing data centres.
- Centre-group IDs must be non-empty and unique. A centre group must contain at
  least two distinct, existing data groups.
- Removing a data centre or data group is rejected while a higher-level group
  still references it.

## Link and routing rules

- Data-centre links are represented at both endpoints.
- Full-mesh links create reciprocal peer edges between all live nodes on both
  sides.
- Relay links record the local relay from each endpoint's perspective.
- Relay failover removes the old cross-link, installs the new reciprocal link,
  and updates both endpoint records.
- A node that is currently an active relay cannot be removed until the link is
  failed over or disconnected.
- Disconnecting data centres removes the cross-centre peer edges and both link
  records.
- A route is returned only when both source and destination nodes exist and are live.
- Graph operations use the data-centre/node map keys as canonical IDs rather than trusting mutable public `Node.id` fields.
- Validation rejects unknown peers, asymmetric peer edges, duplicate node IDs,
  missing reverse links, missing link state, invalid relay references, and
  invalid group membership.

## Persistence and API boundary

The topology types now implement Serde serialization and equality so a future
persistent store can round-trip and validate the complete graph. This change
does **not** introduce a disk store, signed topology snapshots, a node HTTP
management API, cross-node replication, or a distributed consensus protocol.
Those are still required before these operations can be treated as a production
network control plane.

## Verification added

Unit tests cover bidirectional node insertion, duplicate identity rejection,
distinct group membership, reciprocal relay metadata, relay failover, safe
disconnect/removal, dependent-group removal, serialization round-trip, route
symmetry, unknown-node rejection, future heartbeat timestamps, canonical map-key handling, offline-route rejection, node health and
metric updates, and invalid asymmetric topology.
