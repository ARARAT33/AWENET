# Phase 2: automatic Founder network-control snapshot sync

The product runtime now has a bounded, one-way snapshot replication path over the
existing authenticated and encrypted AWE TCP data stream.

## Trust and direction

- Only a local Founder runtime sends snapshots.
- The Founder sends only to peers whose AWEID and public key are proof-backed
  members of its current topology.
- A follower accepts snapshot chunks only from the Founder public key pinned
  out of band in AWE_NETWORK_FOUNDER_PUBLIC_KEY.
- The pinned Founder is admitted as the follower's control-plane transport peer
  even before a Founder Node record exists in the follower topology.
- The receiver independently verifies the Founder-signed envelope and then
  applies the existing Node-local checkpoint and bootstrap-marker rules.
- The follower returns an application acknowledgement only after the snapshot
  and both local rollback anchors have been committed.

The encrypted transport's DataAck only confirms inbox admission. The separate
stream-204 acknowledgement confirms application of the verified snapshot.

## Wire and resource limits

- Stream: 204 (NETWORK_CONTROL_SYNC_STREAM).
- Message version: 1.
- Maximum encoded message: 64 KiB.
- Chunk payload: 12 KiB raw bytes, hex encoded.
- Maximum signed snapshot transferred by this protocol: 16 MiB.
- Maximum chunk count: 1,366.
- One partial transfer is retained per follower, from its pinned Founder only.
- Partial transfers expire after 60 seconds of inactivity.
- Founder retries a transfer after 30 seconds if no matching application ACK
  arrives.
- Duplicate identical chunks are harmless; conflicting duplicates, lower
  sequences, same-sequence equivocation, inconsistent metadata, and transfer
  digest mismatches are rejected.

The local network-control file format still permits 64 MiB. Files larger than
16 MiB remain usable through the existing operator-managed provisioning path
but are not sent by this first automatic sync protocol.

## Policy migration

New default policies allow stream 204. On startup, policy::load_or_create
migrates only the exact previous default allowlist by adding stream 204. A
custom allowlist is not expanded; operators must explicitly add 204 if they
want automatic Founder snapshot delivery.

## Durability ordering

The follower verifies the complete snapshot bytes and all metadata before
writing. The exact signed envelope is atomically persisted only after the
candidate passes the local high-water checks. The v2 bootstrap marker is then
written before the rollback checkpoint, and only after both writes succeed is
the in-memory control plane replaced. A crash between writes leaves a newer
valid snapshot and at least one local signed high-water anchor for startup
reconciliation.

## Remaining limitations

This is single-Founder replication, not consensus or multi-writer conflict
resolution. It does not relay snapshots through intermediate Nodes, does not
protect against full data-directory rollback or local Node identity compromise,
and does not yet replicate User Registry sessions or Site manifests. Delivery
is retried until the follower returns an application ACK; there is no durable
cross-process sender outbox beyond the persisted Founder snapshot itself.
