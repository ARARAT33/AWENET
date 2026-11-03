# Developer API services

`core/src/developer_api.rs` provides the authorization boundary for developer-facing APIs.
It is deliberately independent from the HTTP server so the same policy can be used
by desktop, CLI, and future SDK transports.

## Security contract

- The registry never stores bearer tokens. The transport hashes a presented token
  and passes only its 32-byte digest to the core registry.
- Every key has explicit scopes: `read`, `write`, `storage`, `network`, or
  `diagnostics`.
- Requests are limited in an explicit tick window. Counters saturate and the
  registry has a hard maximum of 4096 keys.
- Revoked and unknown keys fail closed as `UnknownKey`; missing scopes return
  `Forbidden`; exhausted windows return `RateLimited`.
- Key metadata contains labels and audit counters, but no secret material.
- The registry is serializable for durable operator state and deterministic tests.

## Integration boundary

The node HTTP layer should hash the API key at the request boundary, resolve the
endpoint's required scope, call `authorize`, and return `401`/`403`/`429` without
performing the operation when authorization fails. The registry itself does not
invent authentication, expose a network listener, or accept raw tokens.

The current branch adds this reusable core contract and tests. Wiring it into a
public HTTP endpoint requires choosing durable key provisioning and operator
identity flows first; exposing an unauthenticated developer-key creation route
would violate the fail-closed contract.
