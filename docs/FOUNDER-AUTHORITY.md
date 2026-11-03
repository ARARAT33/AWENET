# Founder Authority and Genesis

This document describes the first implementation slice in `core/src/founder.rs`.
It does not claim that a complete login UI, remote notification channel, or
multi-node durable replication is already integrated.

## Genesis rules

- The reserved founder username is `AWENeter`.
- Genesis is created once with `FounderGenesis::initialize_once`.
- The operator must provision a cryptographically random bootstrap secret of
  at least 32 bytes out of band. The public login page must never contain that
  secret or the expected digest.
- The expected digest is computed with `bootstrap_digest` and stored in a
  protected deployment/installation configuration.
- The genesis file is created with exclusive `create_new`; concurrent
  initializers cannot both create the same file.
- The founder public key, AWEID, network ID, initial rules, and signature are
  included in the record. Every node must call `FounderGenesis::verify`
  before trusting it.

A public internet page that lets the first visitor click "Create Founder"
without a pre-provisioned secret is vulnerable to a first-run race. The
bootstrap secret is therefore a security prerequisite, not an optional UX
step. Back up the genesis record and founder private key securely. Loss of
the founder private key cannot safely be repaired by silently electing a
replacement founder.

## Device admission

The genesis founder key is the first approved device. A new device creates a
pending `DeviceRequest` and must first sign its nonce-bound proof message with
its own private key using `device_proof_message` / `prove_device_request`.
Only after proof-of-possession succeeds may an already-approved device sign
the exact request and decision using `approval_message`. The founder may set
the approval policy to require every currently approved device or a selected
set of approved devices; the request snapshots its required approvers, and
approval remains pending until all required signatures are present. Device
block/unblock commands are also signed with `device_control_message`; callers
must not trust an actor-device ID supplied without its signature. The final
approved device cannot be blocked, preventing an accidental permanent
administrative lockout. Offline requests remain
pending in the caller's durable state. Denial does not create an approved
device. Device state and audit history must be persisted atomically by the
integrating application; the in-memory registry alone is not durable.

## Governance updates

`SignedGovernanceUpdate` binds a monotonically increasing sequence number,
the previous update hash, and the new rules. Nodes must verify the signature
against the founder public key pinned in genesis, and reject sequence gaps,
replays, or an unexpected previous hash. Updates must be distributed to
online nodes and queued for offline nodes by the network synchronization
layer.

## Required integration before production

1. Persist genesis, device registry, pending approvals, governance chain, and
   audit records using crash-safe atomic writes and backups.
2. Add a real login flow that unlocks the founder key locally; never transmit
   private keys or passwords to nodes.
3. Deliver signed device-approval prompts to all currently approved devices.
   A request remains pending when the founder is offline.
4. Enforce founder signatures and device approval on every privileged API,
   network control operation, Neter appointment, and ONECOIN policy change.
5. Freeze high-risk actions on suspicious sessions; implement rate limits,
   notification delivery, session revocation, recovery codes, and a second
   recovery key without bypassing device approval.
6. Add multi-process, crash-recovery, race, replay, cross-platform, and
   independent-node integration tests. A passing unit suite is not proof of
   a secure distributed deployment.

## Current implementation boundary

The Rust core currently provides signed genesis creation/verification,
bootstrap-secret validation, exclusive one-time file creation, bounded
device requests, existing-device signed approval/denial, device blocking,
bounded audit records, and chained founder-signed governance updates.
The desktop UI, notification transport, durable replicated device state,
Neter capability integration, and privileged-operation enforcement remain
integration work. Do not expose production founder initialization until the
bootstrap secret and durable storage path are provisioned securely.
