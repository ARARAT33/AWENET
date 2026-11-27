# Durable follower network-control rollback checkpoint

When a Node is configured with both `AWE_REQUIRE_ADMITTED_PEERS=1` and
`AWE_NETWORK_FOUNDER_PUBLIC_KEY`, it keeps a Node-local checkpoint at
`network-control.checkpoint.json` beside `network-control.json`.

The checkpoint binds:

- the pinned Founder AWEID and public key;
- the local Node AWEID and public key;
- the highest accepted snapshot sequence and timestamp;
- a BLAKE3 digest of the exact snapshot string covered by the Founder signature;
- a signature made by the local Node identity.

A separate `network-control.checkpoint.bootstrap.json` marker records that
the one-time enrollment has been consumed and mirrors the accepted high-water
sequence, timestamp and exact snapshot digest. It is signed by the local Node
and bound to the same Founder/Node identity pair. Version-1 markers from the
initial rollout are accepted and upgraded from the separately signed checkpoint
on the next valid startup.

The marker and checkpoint are independent signed anchors. If only the
checkpoint is replayed to an older valid copy, the newer marker prevents the
rollback and the checkpoint is repaired only when the supplied Founder snapshot
matches the marker's high-water value. If only the marker is replayed, the newer
checkpoint repairs the marker. If a process stops between the two atomic writes,
the higher valid anchor is retained and the lagging file is reconciled against
the currently verified Founder snapshot. Equal-sequence disagreement in
timestamp or digest fails closed. A consumed marker without a checkpoint still
returns `CheckpointBootstrapConsumed`; it never silently reopens enrollment.
Do not copy the marker between Nodes: its signature and embedded identity pair
are specific to the local Node that completed enrollment.

Both files use the existing atomic manifest writer. The marker is persisted
before the checkpoint whenever the high-water value advances. A Founder-signed
snapshot older than the higher of the two local anchors is rejected; a
different snapshot at the same sequence is rejected as equivocation. A checkpoint
or marker signed by another local Node, corrupted, oversized, or bound to another
Founder fails closed. A missing checkpoint is never silently recreated after
initial enrollment.


## First startup and existing followers

Every follower requires an explicit one-time checkpoint bootstrap, including
new sequence-zero followers. After verifying the current Founder-signed
snapshot out of band, set
`AWE_NETWORK_ALLOW_CHECKPOINT_BOOTSTRAP=1` for the first controlled startup.
Unset the variable immediately afterward. The process logs a warning whenever
this override is enabled. The same one-time procedure applies to existing
followers whose data directory predates the checkpoint feature.

Do not use the override to recover from a stale-snapshot rejection: doing so
would intentionally reset the local rollback baseline. A malformed or
present-but-invalid checkpoint is never silently replaced. The bootstrap marker
is written before the initial checkpoint so an interrupted enrollment cannot
silently reopen the bootstrap path. If startup reports
`CheckpointBootstrapConsumed` before the first checkpoint was created, verify
the Founder snapshot and local Node identity out of band, then remove only the
bootstrap marker and repeat the one-time enrollment deliberately.

## Security boundary

This protects against replaying an older but correctly Founder-signed snapshot
after an ordinary Node restart, and against accidental deletion or corruption
of the checkpoint. It is not a hardware monotonic counter: an attacker able to
restore or replace the entire local data directory, including the Node identity
and checkpoint, can roll the local trust state back. Stronger protection
requires an OS/hardware-backed monotonic store or an independently replicated
quorum.

A local Founder Node (no external Founder key configured, or an explicitly
pinned key equal to its own Node public key) is not a follower: it applies its
own signed mutations directly to its active control plane and does not run the
follower checkpoint refresh loop. It does not need a follower checkpoint.
`AWE_NETWORK_ALLOW_CHECKPOINT_BOOTSTRAP` is ignored in local-Founder mode; it
is only relevant when the pinned Founder key belongs to another Node.

The checkpoint does not implement network snapshot transport. Founder snapshot
delivery remains operator-managed in this phase.
