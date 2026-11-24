# Phase 2 — cryptographic node admission

**Date:** 2026-11-24

## Purpose

A topology node must not be created by an Admin using an arbitrary string ID. A node admission now requires two independent proofs:

1. The joining Node signs a domain-separated proof of possession with the private key corresponding to its public key.
2. An active Founder/Admin account signs the complete topology action through the existing network-control authorization path.

## Action

```json
{
  "action": "admit_node",
  "centre_id": "dc-west",
  "public_key": [/* 32 public-key bytes */],
  "proof_signature": [/* 64 signature bytes */]
}
```

The Node ID is derived from the public key using the AWEID derivation. It is not accepted as a caller-selected string.

## Node proof message

The node_admission_message helper constructs the exact bytes:

1. UTF-8 domain separator AWENET-NODE-ADMISSION/v1 followed by a NUL byte.
2. Founder AWEID length (u16, big-endian), then Founder AWEID UTF-8 bytes.
3. Data Centre ID length (u16, big-endian), then Data Centre ID UTF-8 bytes.
4. Derived Node AWEID length (u16, big-endian), then Node AWEID UTF-8 bytes.
5. Node public key (32 bytes).
6. Expected next network-control sequence (u64, big-endian).
7. Mutation timestamp (u64, big-endian).

The node proof is bound to the founder trust root, target Data Centre, node identity, exact next sequence and timestamp. A proof from a prior sequence or a different Data Centre cannot be replayed as a new admission.

The Admin/Founder then signs the complete NetworkAction::AdmitNode with the existing network-control signing method. The existing HTTP route POST /api/network/control validates both signatures and persists the signed topology snapshot before publishing the mutation in memory.

## Authorization

- Active Founder: may use AdmitNode and legacy AddNode for manual/legacy topology placeholders.
- Active Admin: may use AdmitNode only; arbitrary legacy AddNode is rejected as a protected action.
- Regular User: cannot mutate network topology.
- Missing, expired, revoked or suspended sessions: rejected by the existing AWEID session guard.

## Stored metadata and compatibility

Admitted topology nodes persist their public key and admission timestamp. The Node ID is re-derived from the public key during topology validation. Legacy/manual topology records omit both optional fields and retain their previous serialized shape.

## Scope boundary

This is cryptographic identity admission into the local signed topology control plane. The repository already has an authenticated encrypted TCP peer handshake in `core/src/network.rs`; this admission action does not yet bind that transport's peer acceptance/routing to topology membership. It also does not prove ongoing reachability, authorize resource contribution, or replicate the control-plane/User Registry trust root to other nodes. Those remain separate Phase 2 tasks.

## Joining-node CLI proof

The joining Node can create the proof without exposing its private key:

```sh
awe-node admission-proof <secret-file> <founder-awe-id> <centre-id> <next-sequence> <at-unix>
```

Example:

```sh
awe-node admission-proof ~/.awep2p/node.awesecret FOUNDER_AWEID dc-west 12 1795520000
```

The command reads the local `.awesecret`, derives the Node AWEID/public key, signs the exact admission message, and prints JSON containing an `action` object ready to include in the Admin-signed `POST /api/network/control` request. It never prints the private key. The Admin/Founder still has to sign the complete action; the node proof alone cannot authorize a topology mutation.

Fetch the current `next_sequence` from the network-control response and use the exact same `at_unix` value in the outer mutation. If another mutation commits first, generate a fresh proof.

## HTTP wire encoding

The JSON request uses byte arrays for both public-key fields:

- `public_key`: 32 integer values in the range 0–255.
- `proof_signature`: 64 integer values in the range 0–255.
- The outer Admin/Founder `signature` remains the existing 128-character hexadecimal string accepted by the HTTP endpoint.
- `at_unix` is Unix time in seconds. The joining node signs the exact value submitted in the mutation.
- The node reads `next_sequence` from the current network-control response and signs that exact sequence. If any topology mutation commits first, it must fetch the new sequence and sign again.

Example outer request shape:

```json
{
  "actor_awe_id": "ADMIN_AWEID",
  "action": {
    "action": "admit_node",
    "centre_id": "dc-west",
    "public_key": [/* 32 bytes */],
    "proof_signature": [/* 64 bytes */]
  },
  "at_unix": 1795520000,
  "signature": "128_HEX_CHARACTERS"
}
```
