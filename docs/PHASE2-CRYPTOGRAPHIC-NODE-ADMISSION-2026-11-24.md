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

This is cryptographic identity admission into the local signed topology control plane. It does not by itself prove that the remote process is currently reachable, establish a peer transport session, authorize resource contribution, or replicate topology to other nodes. Those remain separate Phase 2 tasks.
