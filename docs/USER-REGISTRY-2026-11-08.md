# AWEID user registry and founder-managed accounts — 2026-11-08

## Scope

The node now maintains a durable local account directory keyed by immutable AWE IDs.
AWE IDs are derived from Ed25519 public keys; registration requires a signature from
the corresponding private key. The local node identity is installed as the founder
account on first run. The founder signs role and status mutations.

This is a local control-plane registry, not yet a globally replicated identity
service. It does not use email addresses, phone numbers, or third-party account IDs.

## HTTP endpoints

All endpoints are served by the existing loopback UI API and therefore retain its
local Host/Origin checks.

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/users` | List public account records, current/next sequence, and server time |
| GET | `/api/users/{awe_id}` | Read one account by 64-character AWE ID |
| POST | `/api/users/register` | Register a new identity with proof of possession |
| POST | `/api/admin/users/role` | Founder-signed role change |
| POST | `/api/admin/users/status` | Founder-signed active/suspended change |

Registration body:

```json
{
  "username": "alice",
  "public_key": "<64 lowercase hex characters>",
  "signature": "<128 lowercase hex characters>"
}
```

The registration signature covers these exact bytes, in order:

1. UTF-8 context `AWENET-USER-REGISTER/v1\0`
2. Username byte length as unsigned 64-bit big-endian
3. Username UTF-8 bytes
4. The 32 raw public-key bytes

The server validates the username with the existing `Username` rules, verifies the
signature, derives the AWE ID from the public key, and enforces case-insensitive
username uniqueness. Repeating the exact same registration is idempotent.

Founder mutation bodies contain `awe_id`, `at_unix`, `signature`, and either
`role` (`founder`, `admin`, `user`) or `status` (`active`, `suspended`).

The mutation signature covers:

1. UTF-8 context `AWENET-USER-MUTATION/v1\0`
2. Each of `action`, canonical AWE ID, and new value as a 16-bit big-endian byte
   length followed by its UTF-8 bytes
3. The next registry sequence as unsigned 64-bit big-endian
4. `at_unix` as unsigned 64-bit big-endian

The client reads `sequence`, `next_sequence`, and `server_time_unix` from
`GET /api/users`, then signs the next sequence using a timestamp based on the
node clock. A stale or replayed mutation fails after the registry sequence advances.
Timestamps may not move backwards for the target account.

## Persistence and integrity

- The registry is stored at `users.json` inside the node data directory.
- The full snapshot is signed by the installed node/founder identity.
- Writes use a unique OS-random temporary file, `create_new`, file sync, and atomic
  rename; Unix temporary files are mode `0600`.
- The in-memory candidate is published only after the signed snapshot is persisted.
- A corrupt snapshot or one signed by a different founder causes startup to fail
  closed; the node does not silently replace it with an empty registry.
- The founder account cannot be demoted or suspended, and another identity cannot
  be promoted to Founder.

## Explicit limitations

- The registry is local to this node. Replication, conflict resolution, and
  cross-node account discovery are not implemented by this change.
- `Admin` is currently an account role label only. It does not yet grant API
  capabilities; all role/status changes remain founder-signed.
- `Suspended` is recorded and returned by the directory, but is not yet enforced
  by a shared authentication/session middleware on every network/API operation.
- This does not complete API-key authentication, Session-to-user binding, or the
  capability authorization layer. The local Origin guard is not a substitute for
  authentication against other local processes.
- No browser UI for signing registration or founder mutations is included yet;
  the HTTP contract is usable by a client that can produce the documented
  Ed25519 signatures.

## Verification added

Core tests cover founder immutability, identity proof-of-possession, duplicate
username rejection, founder-only role/status changes, stale timestamps, sequence
exhaustion, signed snapshot round-trip, tamper/wrong-founder rejection, and
temporary-file cleanup.

The three-process product smoke test exercises the HTTP registration and admin
routes, rejects a non-founder signature, verifies the persisted signed snapshot,
and forces a registry rename failure to confirm that the API returns HTTP 500
without publishing the candidate account in memory.
