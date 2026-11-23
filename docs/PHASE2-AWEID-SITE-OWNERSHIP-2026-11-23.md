# Phase 2: AWEID-owned site management

Date: 2026-11-23

## API contract

The following site-management routes now require an active node-local AWEID
Bearer session:

- `GET /api/sites`
- `GET /api/sites/{domain}`
- `POST /api/sites/publish`
- `DELETE /api/sites/{domain}`

The token is issued by the existing proof-of-possession flow:
`POST /api/auth/challenge` followed by `POST /api/auth/login`.

Missing, malformed, expired, revoked, or inactive-account sessions receive
`401 Unauthorized`. A valid session for a different AWEID receives
`403 Forbidden` when it attempts to read, update, or unpublish another
account's site.

## Ownership and publisher identities

New site manifests distinguish the account owner from the node that stores
and publishes the content:

- `owner_awe_id` identifies the authenticated account.
- `owner_public_key` is the account's Ed25519 public key. The core verifies
  that the AWEID is derived from this key.
- `owner_key` remains the local node publisher/signing key for compatibility
  with the existing manifest verification model.

The ownership fields are covered by the manifest signature. A client cannot
supply or override them: the API derives them from the validated session's
current User Registry account.

The lock order for management operations is User Registry -> User Login ->
Site Host. Registry/login guards remain held until the operation completes,
so account suspension or session revocation cannot race a site mutation.

## Existing manifest compatibility

Older manifests have no AWEID owner fields. Their serialization omits the
new optional fields when empty, preserving the prior signing byte sequence.
Such legacy sites remain manageable only by the local Founder AWEID whose
public key matches the legacy manifest publisher key. The system does not
guess an owner or silently transfer a legacy site to another account.

## Public delivery boundary

Public content delivery through the site-serving routes remains separate from
the private management API. It does not expose the local control API through
a site origin. This change does not introduce cross-node manifest replication,
remote ownership transfer, domain registration consensus, or durable
cross-node session state. Sessions remain node-local and are invalidated by a
node restart.

## Validation added

- Core test binds a new manifest to a real AWEID/public-key pair and verifies
  that changing the owner invalidates the signature.
- Core test deserializes a legacy manifest without the new fields and verifies
  that its existing signature remains valid.
- Three-node product smoke now verifies unauthenticated management rejection,
  authenticated publish/list/get/update/unpublish, a second AWEID publishing
  on the same node, and cross-account read/delete rejection.
