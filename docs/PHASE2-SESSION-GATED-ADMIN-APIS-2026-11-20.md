# Phase 2: Session-gated Admin and Network APIs

Date: 2026-11-20

This change builds on the AWEID proof-of-possession and node-local bearer session API introduced by the stacked parent PR.

## Enforcement added

The following mutating endpoints now require a valid `Authorization: Bearer <token>` session:

- `POST /api/admin/users/role`: requires an active session for the installed Founder AWEID. The existing Founder Ed25519 signature remains mandatory.
- `POST /api/admin/users/status`: requires a session whose AWEID matches `actor_awe_id` and whose role is Founder or Admin. The existing actor signature and registry-sequence checks remain mandatory. When `actor_awe_id` is omitted, only the installed Founder session is accepted.
- `POST /api/network/control`: requires a session matching `actor_awe_id` for Founder/Admin topology mutations. When no actor is supplied, only the installed Founder session is accepted. Existing Ed25519 action signatures, capability checks, sequence checks, topology validation, and founder-signed durable snapshots remain in force.

A valid session alone never authorizes an operation: the endpoint still checks the role/capability and the action-specific signature.

## Concurrency and revocation

The mutation routes use the lock order:

1. User Registry
2. User Login
3. Network Control (for topology mutations)

The Registry and Login guards remain held through the durable mutation. This prevents a concurrent account suspension, role change, or logout from racing between session validation and the privileged write.

Invalid, expired, suspended, sequence-stale, or actor-mismatched sessions are rejected. Session validation is node-local; a token issued by one node is not valid on another node.

## Regression coverage

The three-node product smoke now checks:

- role mutation without a bearer token is rejected;
- a Founder session plus an invalid signature is rejected;
- a valid Founder session plus a Founder signature can promote an account;
- an Admin session can moderate a regular User only when its AWEID matches the signed actor;
- an Admin session cannot promote roles or target protected accounts;
- suspending an Admin invalidates its existing session;
- network mutation without a session is rejected;
- a valid session for the wrong actor is rejected;
- an active Admin session can create a Data Centre through the signed topology API;
- an ordinary User session cannot change topology;
- Admin telemetry mutation remains forbidden;
- Founder-signed topology operations continue to work with a Founder session.

## Explicit boundary

This change gates the privileged User Registry and Network Control mutation endpoints. It does not yet apply the same session/capability middleware to Site Hosting ownership, Node admission, federation import, group/channel management, storage mutation, or every other private API. Those routes must be reviewed and migrated individually rather than assumed protected.

Sessions remain in-memory and node-local in the parent PR; restarting a node invalidates its sessions. No new dependencies are introduced.
