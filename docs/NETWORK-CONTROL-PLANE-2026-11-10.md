# Durable network control plane — 2026-11-10

## Scope

This increment turns the topology model from a mutable in-memory structure
into a founder-authorized local control-plane primitive. It is intentionally
stacked on PR #111, which supplies the topology membership and routing
invariants.

## Implemented

- A versioned `NetworkControlPlane` owns the topology, founder AWEID/public
  key, monotonic mutation sequence and last committed timestamp.
- Every mutation is represented by a typed `NetworkAction` and a
  domain-separated founder signature over the exact action, next sequence and
  timestamp.
- The control plane validates the installed founder identity, signature,
  monotonic timestamp, bounded clock skew, action size and resulting topology.
- Node, Data Centre, Data Group and Centre Group lifecycle operations are
  available through the typed action API, including connect/disconnect,
  relay failover, health/metrics updates and dependency-safe removal.
- Mutations run against a candidate topology. The candidate is validated
  before it replaces live state.
- `apply_and_save` persists the candidate before publishing it in memory;
  a failed durable write leaves the previous in-memory state unchanged.
- Snapshots are wrapped in a versioned founder-signed envelope. The signature
  covers the exact serialized snapshot string, avoiding unstable reserialization
  of HashMap-backed topology structures.
- Snapshot writes use the existing atomic storage writer (CSPRNG temporary
  name, create_new, write/sync, atomic rename, Unix 0600 and best-effort
  parent-directory sync).
- Loads are bounded to 64 MiB, verify the installed founder and outer
  signature, and run full topology validation. Existing corrupt or
  differently-owned state is never silently reset.

## Signing contract

The mutation signing message is:

1. `AWENET-NETWORK-CONTROL-MUTATION/v1\0`
2. control-plane version (u16 big-endian)
3. founder AWEID length (u16 big-endian) and UTF-8 bytes
4. next sequence (u64 big-endian)
5. action timestamp (u64 big-endian)
6. action JSON length (u32 big-endian)
7. serialized `NetworkAction` JSON bytes

Clients must obtain the current sequence and construct the message against the
same snapshot. A concurrent mutation changes the next sequence and makes an
older signature unusable.

## Current boundary

This PR provides the core durable mutation engine and tests. It does not yet
add HTTP endpoints, bind user sessions/capabilities to network mutations, or
replicate snapshots between independent physical nodes. The runtime integration
must call `apply_and_save` only after endpoint authorization and must not expose
the founder private key to remote clients. These are explicit follow-up tasks,
not claimed as complete here.

## Regression tests

- founder signature required and sequence-based replay rejected;
- signed creation of centres/nodes and Data Group membership;
- Centre Group requires two existing Data Groups;
- invalid membership leaves sequence and topology unchanged;
- signed snapshot round-trip, wrong-founder rejection and tamper detection;
- failed durable commit leaves live state and disk directory unchanged;
- monotonic and bounded mutation timestamps;
- corrupt existing snapshots fail closed;
- sequence exhaustion is rejected before signing or mutation.

No local Cargo result is claimed. Exact-head Linux and Windows workspace
build/tests, Clippy, formatting, architecture checks and Security Audit are
required before this stacked PR can be considered merge-ready.
