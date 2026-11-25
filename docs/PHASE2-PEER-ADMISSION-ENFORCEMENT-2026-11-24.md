# Phase 2 — enforce cryptographic node admission at the P2P transport

**Date:** 2026-11-24

## Purpose

The cryptographic admission action in the preceding change recorded a proof-backed node in the signed local topology, but the TCP transport still accepted any peer with a valid self-signed AWEID handshake. This change connects those two layers.

## Enablement

Set the following environment variable before starting `awe-node`:

```sh
AWE_REQUIRE_ADMITTED_PEERS=1
```

The value is case-insensitive and also accepts `true` or `yes`. When unset or false, the existing open-peer behavior remains for backward compatibility.

PowerShell on Windows:

```powershell
$env:AWE_REQUIRE_ADMITTED_PEERS = "1"
.\awe-node.exe run .\identity.vault "<vault-password>" 0.0.0.0:41000
```

POSIX shells:

```sh
AWE_REQUIRE_ADMITTED_PEERS=1 awe-node run ~/.awep2p/identity.vault "<vault-password>" 0.0.0.0:41000
```

Remove the environment variable (or set it to `0`) to restore the legacy open-peer mode.

The product runtime uses its already-loaded `network-control.json`. The `awe-node run` command loads or initializes the same file beside the supplied identity vault. An existing snapshot signed by a different Founder identity is rejected; it is never silently replaced.

The local status response includes `peer_admission_enforced` so an operator can verify whether this mode is active.

## Follower Node trust root

A standalone follower started with `awe-node run` must verify the same Founder-signed
`network-control.json` snapshot as the network owner. It must not try to load that
snapshot as if the follower's own transport identity were the Founder.

1. Copy the current Founder-signed `network-control.json` into the directory beside
   the follower's identity vault.
2. Set `AWE_NETWORK_FOUNDER_PUBLIC_KEY` to the Founder's 32-byte Ed25519 public key
   encoded as exactly 64 hexadecimal characters. The public key is available in the
   Founder identity's `.awesecret` file as `public_key`; do not copy or disclose
   `secret_key`.
3. Start the follower with `AWE_REQUIRE_ADMITTED_PEERS=1` as shown above.

Example PowerShell:

```powershell
$secret = Get-Content .\founder.awesecret -Raw | ConvertFrom-Json
$env:AWE_NETWORK_FOUNDER_PUBLIC_KEY = $secret.public_key
$env:AWE_REQUIRE_ADMITTED_PEERS = "1"
.\awe-node.exe run .\identity.vault "<vault-password>" 0.0.0.0:41000
```

When the public-key variable is absent, `awe-node run` uses the local Node identity
as the Founder and loads/creates a local control plane. When the variable is present,
the Node loads the existing snapshot through public-key verification; a missing,
corrupt, mismatched or invalidly signed snapshot fails closed and is never replaced.

This is a pinned, read-only follower view. The current release does not push snapshots
over the network: an operator must securely distribute the Founder-signed file. A running
follower now checks that file every five seconds and applies a newer verified snapshot
without restarting the Node.

Snapshot replacement is monotonic. Exact duplicates are ignored; a lower sequence,
a different snapshot with the same sequence, a different Founder, a corrupt file or an
invalid signature is rejected. Rejected updates leave the last verified in-memory
control plane active. The follower logs a changed rejection reason and retries on the
next poll. The shared admission policy observes an accepted update on its next
connection/message/heartbeat check; revocation is not instantaneous.

When transferring an update, copy it to a temporary file in the follower's data
directory, verify the transfer completed, then replace network-control.json atomically
on the same filesystem. Do not expose the Founder private key or accept an unsigned
topology JSON response as a replacement for the signed snapshot.

## Follower snapshot refresh

The standalone Node refreshes its signed control snapshot every five seconds while
peer admission enforcement is enabled. The refresh runs outside the async executor's
worker threads and does not hold the admission-state mutex while reading the file.

Only a snapshot that passes network_control::load_verified against the pinned
Founder public key is considered. Before publication, the active control plane checks:

- The Founder AWEID and public key are unchanged.
- The complete topology still passes structural validation.
- The sequence is strictly newer, unless the snapshot is an exact duplicate.
- The update timestamp does not move backwards.
- A different signed state at the same sequence is rejected as equivocation.

The update is applied to the shared control-plane state only after all checks pass.
A malformed, incomplete or stale file never replaces the last known-good state.
A failed read is retried on the next poll.

Rollback protection currently lasts for the lifetime of the running process.
After a restart, the Node validates the snapshot file that is present at startup; without
an external monotonic counter or separately protected durable high-water record, an
older but correctly signed snapshot could be replayed. Keep the latest Founder-signed
file and its backups access-controlled. Durable anti-rollback state is still required.

This is operator-assisted snapshot distribution with automatic local reload, not
automatic network replication, delta synchronization, quorum consensus or a live
Founder-to-follower push protocol. Those remain separate work.

## Admission rule

A remote peer is admitted only when the local Founder-signed topology contains a node record that:

1. Is keyed by the exact AWEID derived from the peer's authenticated Ed25519 public key.
2. Contains that exact public key.
3. Contains a proof-backed `admitted_at_unix` timestamp.
4. Has not been removed from the topology.

Legacy `AddNode` placeholders do not satisfy this rule. The new `AdmitNode` action is required.

## Enforcement points

The policy is checked:

- On inbound TCP connections, immediately after the authenticated HELLO handshake.
- Again during the inbound connection loop, so a health change or removal is observed on the next message/heartbeat cycle.
- On outbound `Node::connect` and therefore bootstrap/discovery connections.
- Before sending over a cached outbound connection.
- Before pinging a cached outbound connection.

A policy-lock failure is fail-closed. The peer is not admitted.

## Operating model and limits

This is **local topology enforcement**, not yet a replicated network-wide admission consensus protocol.

Every node uses its own local Founder-signed `network-control.json`. For a bidirectional mesh, each endpoint must admit the other endpoint in its own local topology. The joining node proof is bound to that local control plane's Founder AWEID, target Data Centre, next sequence and timestamp.

Removing a node blocks new connections and subsequent operations. Operational health is deliberately separate from membership: a temporarily unhealthy node remains admitted so it can reconnect and recover. An already-open inbound connection is closed when the peer policy is next checked (on a message or heartbeat cycle); this is not an instantaneous distributed revocation protocol.

This change does not yet replicate topology snapshots, User Registry state or trust roots between nodes, and it does not implement remote domain/site consensus or resource-contribution authorization.

## Tests added

- An actual local TCP listener accepts the exact admitted AWEID/public-key pair and completes a heartbeat.
- A different, cryptographically valid peer completes the HELLO handshake but is disconnected before a heartbeat succeeds.
- An outbound connection is rejected when the local node's admission policy denies the authenticated remote identity.
- The control-plane lookup accepts only matching proof-backed active records and rejects mismatched keys, unhealthy records, legacy placeholders and removed nodes.

No dependency changes.
