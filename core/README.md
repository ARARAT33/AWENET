# AWEp2P Core

The shared Rust core provides platform-independent identity, cryptography, protocol, networking, storage, and application primitives.

## Security foundation
The repository includes Ed25519 identity/signature functionality, AWE-ID derivation, username validation, password-protected local vault functionality, Argon2 password verification, authenticated vault encryption, explicit recovery/export paths, transient-secret zeroization, OS randomness, domain-separated hashing, canonical protocol encoding, versioning, and replay-protection primitives.

These components are not an independent security audit or a guarantee of production security.

## Trust boundaries
Treat peer input as untrusted. Validate framing, sizes, versions, signatures, authorization, sequence/replay state, and content integrity. Bound memory, retries, queues, and computational work.

## Persistence and networking
Distinguish local durable state from remote state. A successful local storage operation does not establish distributed persistence. Network features require socket-level and independent-node tests, including disconnect and malformed-input cases.

## Platform contract
Windows, Linux, Android, and future AWEOS clients should reuse the same core semantics while respecting platform-specific storage, lifecycle, and permission requirements.

## Maturity
The core is an experimental foundation. Production use requires current CI evidence, integration testing, platform security review, and documented recovery behavior.