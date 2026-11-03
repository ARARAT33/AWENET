# AWENET Founder Authority and Genesis

## Status

This document defines the security contract for Founder Authority. The first implementation increment is the signed, one-time local genesis primitive in core/src/founder.rs. It does not claim founder login, remote device approval, or network-wide governance enforcement is already integrated.

## Founder identity

- The sole founder account name is AWENeter.
- Founder identity is an AWEID derived from the founder's Ed25519 public key, never from email, phone, or an external account.
- Genesis binds the network ID, founder AWEID, founder public key, protocol version, and authority epoch.
- The founder signs the genesis payload under the AWENET/FOUNDER-GENESIS/v1 domain.

## One-time initialization

1. A trusted release/bootstrap configuration pins a SHA-256 commitment to a randomly generated bootstrap token of at least 256 bits.
2. The founder enters the token during first-run initialization. The token itself is never written to the genesis record.
3. The program verifies the token against the pinned commitment before creating genesis.
4. The genesis file is opened with exclusive create_new. Competing local processes cannot both initialize the same path. On Unix it is created with mode 0600.
5. The record is flushed to disk and verified on every load. A second initialization is rejected.

**Important distributed-systems boundary:** exclusive file creation prevents a local filesystem race; it does not elect a globally unique founder across unrelated public nodes. Every client/node must pin the same bootstrap commitment through a trusted release channel, verify the signed genesis, and reject any different network ID or founder key. A modified client that discards the trust root cannot be forced to obey the protocol. The first public visitor to an unauthenticated web page must never automatically become founder.

## Login and device approval — required integration

- Login name is fixed to AWENeter; passwords are only one factor and never override device approval.
- A device proves possession of its own key and submits a signed, expiring challenge.
- Requests from unapproved devices remain pending while the founder is offline.
- An already approved founder device signs an explicit YES/NO decision bound to the challenge and device public key.
- Denial can apply a temporary or permanent device block; repeated failures trigger rate limits and alerts.
- New device approval, key rotation, recovery, session revocation, and emergency locks must be audited.
- Recovery codes and a second founder-controlled key are recovery factors, not bypasses for device approval.

## Authority and Neters

- Founder-signed policy is the root of authority; community votes and chat groups may submit advice but cannot replace or remove the founder.
- Nodes verify policy signatures and monotonically increasing authority epochs. Unknown, stale, or invalid policies fail closed for privileged operations.
- Neters receive explicit, narrow, revocable capabilities. They cannot appoint other Neters unless the founder grants that capability.
- Every privileged action records actor, target, policy epoch, result, and evidence in an append-only audit stream.

## Remaining implementation gates

1. Persist and replicate a pinned trust anchor and genesis record across nodes.
2. Add signed policy snapshots and anti-rollback epoch checks to the protocol.
3. Implement founder login, local encrypted vault integration, device challenge/approval, recovery, and global session revocation.
4. Enforce founder/Neter capabilities at every administrative HTTP route and service boundary.
5. Add the Founder Control Center and emergency lock workflows.
6. Add independent-node tests: competing genesis, wrong trust anchor, tampered signatures, stale policy replay, offline device approval, denial, key rotation, and recovery.
7. Verify Linux and Windows builds, unit/integration tests, Clippy, UI contract, and security audit in GitHub Actions.

A module, UI screen, or green unit test alone is not production readiness. Release status requires real independent-node evidence and clean-machine recovery tests.
