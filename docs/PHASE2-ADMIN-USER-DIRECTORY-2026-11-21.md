# Phase 2: authenticated Admin user directory

Date: 2026-11-21

## Endpoint

`GET /api/admin/users` returns a bounded page of the node's local AWEID
account registry. It is an administrative directory, not a public user search
endpoint.

Every request must include:

```http
Authorization: Bearer <node-local-session-token>
```

The session must be active and belong to an account whose current role is
`founder` or `admin`. Suspended, expired, revoked, unknown, or regular-user
sessions are rejected. The session is revalidated against the live registry on
every request.

## Pagination

- `limit` is optional and defaults to 50.
- The accepted limit is 1 through 100.
- `after` is an optional 64-character hexadecimal AWEID cursor.
- Results are sorted by AWEID and the cursor is exclusive.
- A non-null `next_cursor` is the final AWEID in the returned page. Pass it as
  `after` to request the next page.
- Unknown or duplicate query parameters, malformed cursors, and out-of-range
  limits return `400 Bad Request`.

Example:

```http
GET /api/admin/users?limit=50
Authorization: Bearer <token>
```

Successful responses include `users`, `total`, `limit`, `next_cursor`,
and the registry `sequence`. Account objects expose public identity and
moderation metadata only; private keys and bearer tokens are never returned.

## Status codes

- `200 OK`: authenticated Founder/Admin page.
- `400 Bad Request`: invalid pagination input.
- `401 Unauthorized`: missing, invalid, expired, or revoked session.
- `403 Forbidden`: valid session without administrative role.
- `500 Internal Server Error`: registry or login state lock failure.

## Consistency and limits

The endpoint uses the same lock order as privileged mutation handlers:
User Registry, then User Login. This keeps session validation tied to the
current account status/role while the page is constructed. Cursor pagination
avoids cloning the entire registry for every page. A page is a point-in-time
view; concurrent registrations or moderation changes may affect subsequent
pages, so clients should restart pagination if the returned registry sequence
changes.

The current session store remains node-local and memory-only. Restart recovery,
distributed account replication, and cross-node administrative authorization
are separate milestones and are not claimed by this endpoint.
