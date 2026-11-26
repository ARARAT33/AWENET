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

Checkpoint writes use the existing atomic manifest writer. A newer checkpoint is
persisted before the verified snapshot is published to the live admission
policy. If persistence fails, the candidate is not applied. On restart, a
Founder-signed snapshot older than the local checkpoint is rejected; a different
snapshot at the same sequence is rejected as equivocation. A checkpoint signed
by another local Node, corrupted, oversized, or bound to another Founder fails
closed. A missing checkpoint also fails closed; it is never silently recreated
after initial enrollment.

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
present-but-invalid checkpoint is never silently replaced.

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

The checkpoint does not implement network snapshot transport. Founder snapshot
delivery remains operator-managed in this phase.
