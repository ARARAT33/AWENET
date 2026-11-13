# ID-addressed site hosting — 2026-11-12

This increment connects `AweHost` to the local `awe-node` HTTP product.
It provides local publishing and serving, not remote replication or global
name resolution.

## HTTP API

- `GET /api/sites` — list signed manifests owned by this node identity.
- `GET /api/sites/{domain}` — inspect a locally hosted domain.
- `POST /api/sites/publish` — publish or update a site.
- `DELETE /api/sites/{domain}` — unpublish the current manifest.
- `GET /site/{site-id}/{path}` — serve a content-addressed version.

Publish JSON contains a valid `domain`, positive `version`, and 1–256 files.
Every site must include `/index.html`. Each file has a normalized `path`,
optional `content_type`, and either UTF-8 `content` or hexadecimal
`data_hex`. Traversal, duplicate paths, invalid MIME values, oversized
files and oversized sites are rejected.

The node identity signs the complete manifest. The signature covers the owner
key, domain, version, file metadata, policy and access descriptor; the root
hash is recomputed on load. Invalid signatures or hashes fail closed.
Updates require a strictly increasing version and the current node identity
must match the existing owner. After a manifest is durably replaced or
unpublished, the node rebuilds the live-object set from every valid persisted
site manifest and removes only unreferenced content-addressed objects. Objects
shared by another site's manifest remain live. If any manifest cannot be
loaded or verified, collection fails closed and reports cleanup as pending;
it does not delete objects based on an incomplete reference set.

## Publish and cleanup response

Successful publish/update and unpublish responses include:
- `garbage_collected_objects`: the number of unreferenced objects removed by this operation.
- `cleanup_pending`: `true` when collection could not build a complete verified reference set or could not finish safely.

A successful site mutation remains committed when cleanup is pending. Clients must not retry the publish/unpublish solely because cleanup is pending; a later successful site lifecycle operation can retry collection. Collection never deletes objects when any persisted manifest fails verification.

## Browser isolation

A site ID is the manifest root hash. Opening `awe://site-<64-hex-id>`
navigates to `http://<site-id>.localhost:<ui-port>/site/<site-id>/index.html`.
That virtual origin can GET only that site's paths. It cannot access node APIs
or another site's content. The desktop Browser iframe uses a sandbox without
top-navigation permission.

## Storage guarantees

Site objects use the node's content-addressed store. Object publication uses
unique create-new temporary files, file sync, atomic rename and best-effort
directory sync. Corrupt existing objects are repaired only after replacement
bytes are verified. Quota statistics count only 32-byte content IDs.

## Limits not claimed

- Local-node hosting only; remote discovery and retrieval are not done.
- Federated domain ownership and transfer are not implemented.
- Site updates create a new content-addressed ID; the previous ID stops
  resolving after the new version is committed.
- Site-object garbage collection runs after publish/update/unpublish; a
  corrupt manifest blocks collection rather than risking deletion of live data.
- The API uses local same-origin/loopback controls, not network-wide
  Session/Capability authorization.


The HTTP listener also checks the actual TCP peer address and serves the
local UI/API only to loopback peers. A caller cannot gain local access by
spoofing `Host: localhost` or an `Origin` header from a remote address.
Remote site delivery must use a separate authenticated network transport;
this local UI listener is not that transport.
- Binary data is hex-encoded and has additional request-body overhead.
