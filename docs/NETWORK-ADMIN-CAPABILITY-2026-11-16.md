# AWEID-scoped network topology administration — 2026-11-16

## Scope

This increment connects the signed AWEID user registry to the existing durable
network topology control plane. It does not replace the founder trust root or
claim remote multi-node consensus.

## Capabilities

Active Founder and Admin accounts receive:

- `UserModerate`: suspend/reactivate ordinary User accounts.
- `TopologyManage`: manage structural topology membership.

Suspended accounts receive no capabilities. Ordinary User accounts receive
neither capability.

## Allowed administrator topology actions

An active Founder/Admin can sign and submit these structural operations:

- Create/remove Data Centres.
- Add/remove node records within a Data Centre.
- Create/remove Data Groups.
- Create/remove Centre Groups.
- Connect/disconnect Data Centres.
- Trigger relay failover.

The server rejects `SetNodeHealth` and `UpdateNodeMetrics` through the
administrator path. Those values are runtime/observed telemetry and must not
be fabricated by a human administrator. The existing founder-signed route
continues to support its established operations.

This API manages topology records. It does not by itself admit a physical
device, prove that a node is online, reserve its resources, or establish a
remote authenticated transport session.

## Signed request

The existing `POST /api/network/control` accepts an optional
`actor_awe_id`. When present, the signature is checked against that account's
public key in the founder-signed User Registry. The signing bytes contain:

1. `AWENET-NETWORK-CONTROL-ADMIN-MUTATION/v1\\0`.
2. Network control version (u16 big-endian).
3. Founder AWEID length and UTF-8 bytes.
4. Actor AWEID length and UTF-8 bytes.
5. Current signed User Registry sequence (u64 big-endian).
6. Next control-plane sequence (u64 big-endian).
7. Mutation timestamp (u64 big-endian).
8. Action JSON length (u32 big-endian) and serialized `NetworkAction`.

The actor identity, current User Registry sequence, action, timestamp and next
control-plane sequence are therefore bound to the signature. A role/status
change invalidates outstanding admin signatures even if the control-plane
sequence has not changed; topology replays also fail after the sequence advances.

The node verifies the actor capability, applies the action to a candidate
topology, validates all topology invariants, and writes the new snapshot before
publishing it in memory. The snapshot remains signed by the installed founder
key and stores `last_actor_awe_id` for operator visibility. Existing founder
clients that omit `actor_awe_id` keep the original founder-signature contract.

## Remaining boundary

- This is a local node control plane, still loopback/origin restricted.
- The founder-signed registry and snapshot remain local durable state.
- Remote admin sessions, cross-node authorization/replication, physical node
  admission and proof-of-resource ownership are not implemented by this change.
- Site ownership still needs to be bound to AWEID rather than only the local
  node identity.
