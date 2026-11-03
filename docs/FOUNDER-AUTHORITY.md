# AWENET Founder Authority

## Implemented cryptographic core

The Founder Authority implementation is split across:

- core/src/founder.rs — one-time signed Genesis, immutable founder rules, device proof-of-possession, approval policies, signed device controls, and chained governance updates.
- core/src/founder_auth.rs — device-bound login challenges, single-use signatures, temporary lockout, expiring/revocable sessions, suspicious-session restriction, global logout, and emergency-lock-aware login entry point.
- core/src/founder_admin.rs — founder-controlled Neter applications, appointments, capability-scoped delegation, compensation/status changes, and signed two-level emergency locks.

This is a tested Rust domain core. It is not yet a completed desktop login/control-center UI, remote notification transport, durable replicated state service, or enforcement middleware on every network/admin endpoint.

## Founder identity and one-time Genesis

- The sole founder handle is AWENeter.
- AWEID is derived from the founder's Ed25519 public key, not from email, phone, or external accounts.
- Genesis binds the network ID, founder AWEID, founder authority public key, a separate initial device public key, initial rules, and a founder signature.
- The founder authority key and the initial device key are different key pairs. Possession of the founder authority key alone does not make a device approved.
- Initialization requires an out-of-band random bootstrap secret of at least 256 bits. Its digest must be pinned in trusted installation/release configuration; the raw secret must never be included in public UI code.
- The Genesis file uses exclusive create_new, Unix mode 0600, fsync, and cleanup on write failure. A second local initialization is rejected.

**Distributed boundary:** create_new prevents a local filesystem race, not a global election across unrelated public nodes. Every node must pin the same bootstrap/genesis trust root and reject a different founder key/network ID. A modified client that discards the trust root cannot be forced to obey. Never let the first public visitor automatically become founder.

Production device keys should be non-exportable platform credentials (passkeys/WebAuthn, TPM, or secure enclave) where supported. A software-only private key cannot guarantee physical device binding if an attacker copies that device key too.

## Device approval and login

- A new device signs a nonce-bound proof-of-possession request.
- The founder can require approval from all currently approved devices or a selected set. Each request snapshots the required approvers and remains pending until all required approval signatures arrive.
- Denials, approvals, block/unblock operations, and governance changes are cryptographically bound to their exact statements.
- The last approved device cannot be blocked, avoiding accidental administrative lockout.
- Login challenges are short-lived and single-use. Only an approved, unblocked device can request one; it must sign the challenge with its own key.
- Repeated bad signatures trigger a temporary per-device lockout.
- Sessions expire, can be individually revoked or restricted, and support global logout. Validation checks that the bound device remains approved and unblocked.
- The caller must use the emergency-lock-aware login entry point when applying emergency state. Session tokens are bearer secrets and must never be logged, placed in URLs, or stored unencrypted.

## Founder governance

Founder-signed updates are sequence-numbered and chained to the previous update hash. Nodes must reject stale/replayed updates, sequence gaps, unexpected previous hashes, invalid signatures, and attempts to weaken immutable rules. Community votes and AWEMessenger groups can advise but cannot replace or remove the founder.

The emergency state has two independent signed levels:
- Account lock: blocks founder login.
- Governance lock: blocks governance policy verification/changes through the guarded API.

Emergency commands are signed by the Genesis founder key, sequence-chained, and audited. All login and governance entry points must call the corresponding guard before performing privileged work.

## Neters

- Neters can be appointed directly by the founder or by approval of a signed application.
- Each Neter receives explicit scopes, status, and founder-set compensation.
- A Neter can appoint another Neter only when the founder explicitly grants that capability; delegated scopes cannot exceed the parent Neter's scopes, and the child does not inherit re-delegation authority.
- Only the founder can change compensation, suspend, reinstate, or remove a Neter.
- Neters have no authority over the founder or founder governance.

The current registry is in-memory; integrating services must persist it atomically and authorize every operation with a valid founder session/device.

## Required integration before production

1. Add a real first-run/login UI that unlocks founder authority locally and provisions the bootstrap trust root safely.
2. Store founder and per-device credentials in OS-protected/non-exportable stores where available; do not transmit private keys or passwords to network nodes.
3. Persist Genesis, device registry, pending approvals, login sessions, governance chain, Neter registry, emergency state, and audit logs using crash-safe writes, backup and recovery.
4. Deliver device approval prompts to existing approved devices; offline requests remain pending.
5. Enforce a valid device-bound founder session and founder signatures on every privileged API, node control operation, Neter action, governance change, and ONECOIN policy update.
6. Integrate recovery codes and a second recovery key without bypassing device approval.
7. Add cross-platform restart, concurrent-init, replay, lockout, device revocation, emergency lock, Neter delegation, and independent-node tests.

A passing unit suite or a visible UI screen alone is not production readiness. Release requires Linux/Windows tests, Clippy, security review, clean-machine recovery tests, and independent-node verification.
