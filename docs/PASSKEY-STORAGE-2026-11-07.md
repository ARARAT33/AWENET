# Passkey storage product layer

AWENET founder passkey enrollment state is persisted in `founder-passkeys.json` next to the node data directory. The file contains only public passkey credentials, recovery-code hashes, and founder guard metadata; passkey private material remains in the platform authenticator and never enters the node process.

## Integrity and durability

The persisted guard is signed by the local founder identity. Startup accepts the file only when the format version, founder AWE-ID, founder public key, and signature all validate. A different founder or any byte-level tampering is rejected closed rather than silently resetting security state.

Writes use a unique `create_new` temporary file, mode `0600` on Unix, `sync_all`, atomic rename, and parent-directory sync. A failed write leaves the previous valid file untouched. The first startup creates and persists an empty founder guard.

## Restart behavior

MFA step-up sessions and pending challenges are intentionally not durable. They are cleared before signing a snapshot and again after loading, so a process restart cannot resurrect an authenticated founder session. The enrollment list and single-use recovery-code state remain durable.

The local API endpoint `GET /api/security/passkeys` exposes bounded status only: enrolled count, credential labels, recovery-code count, step-up activity, and the persistence mode. It never returns public-key bytes, recovery hashes, signatures, or private credentials.
