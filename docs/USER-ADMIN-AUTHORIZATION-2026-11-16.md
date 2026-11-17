# AWEID administrator moderation

This increment makes the existing `Admin` account role operational for one narrowly
scoped task: suspending and reactivating ordinary `User` accounts.

## Capability resolution

`UserRegistry::capabilities_for(awe_id)` returns no capabilities for suspended
accounts. Active `Admin` and `Founder` accounts receive `UserModerate`; active
ordinary users do not.

The HTTP moderation operation additionally restricts its target to an account
whose role is exactly `user`. It cannot modify a founder or another administrator.
Role assignment and changes to privileged accounts remain founder-signed.

## HTTP request

`POST /api/admin/users/status`

```json
{
  "actor_awe_id": "<active administrator AWEID>",
  "awe_id": "<ordinary user AWEID>",
  "status": "suspended",
  "at_unix": 1790000000,
  "signature": "<128 hexadecimal characters>"
}
```

Set `status` to `active` to reactivate an ordinary account. The response includes
the updated user, registry sequence and `actor_awe_id`.

For backward compatibility, omitting `actor_awe_id` retains the existing
founder-signed status mutation. The endpoint does not infer an actor from a
client-supplied username or role.

## Signature format

The Ed25519 signature covers these bytes:

1. Domain separator: `AWENET-USER-ADMIN-MUTATION/v2\0`.
2. Five UTF-8 fields, each preceded by its byte length as unsigned 16-bit
   big-endian: `set_status`, founder AWEID, actor AWEID, target AWEID, and
   status string.
3. The next registry sequence as unsigned 64-bit big-endian.
4. `at_unix` as unsigned 64-bit big-endian.

The server resolves the actor's public key from the signed user registry and
checks that the account is active and has `UserModerate`. The target must be an
ordinary user, and the timestamp cannot precede the target's last update or
exceed the server's current Unix time by more than five minutes. The core API
receives `now_unix` from its caller rather than reading a platform wall clock,
so validation remains deterministic and usable by non-native clients; the HTTP
server supplies its own current time when applying the mutation. Binding the
founder AWEID prevents a valid administrator signature from being replayed
against another founder-owned registry with matching actor, target and
sequence. Including the registry sequence also makes an accepted signature
unusable for a second mutation.

## Security boundary and remaining work

This is signed-request authorization, not a general bearer-session system.
The current node HTTP UI API remains loopback/origin restricted. General AWEID
login sessions, shared capability middleware across every API, remote
administrative access, audit export, and capability enforcement for node,
data-centre, data-group, centre-group, network and site operations remain
separate Phase 2 work.
