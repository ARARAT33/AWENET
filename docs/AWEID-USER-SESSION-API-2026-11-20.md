# AWEID User Login and Node-Local Sessions

Date: 2026-11-20

This document describes the first HTTP integration of the core AWEID proof-of-possession session manager.

## Endpoints

### `POST /api/auth/challenge`

Request:

```json
{
  "awe_id": "<64-character AWEID hex>",
  "ttl_secs": 120
}
```

The node issues one outstanding challenge per active AWEID. A new challenge for the same account invalidates its previous outstanding challenge. The response contains `challenge_id`, `nonce`, issue/expiry timestamps, the account sequence, and `message` (hex-encoded canonical bytes to sign).

The client signs the decoded `message` bytes with the private key corresponding to the AWEID.

### `POST /api/auth/login`

Request:

```json
{
  "challenge_id": "<challenge id hex>",
  "signature": "<Ed25519 signature hex>",
  "session_ttl_secs": 3600
}
```

The challenge is consumed exactly once. The node verifies the signature and confirms that the account remains active with the same public key and account-local sequence. A successful response returns an opaque 32-byte random bearer token as hex.

### `GET /api/auth/session`

Send:

```http
Authorization: Bearer <token hex>
```

The node validates the token, expiry, active account status, and account-local sequence, then returns the resolved public account record.

### `POST /api/auth/logout`

Send the same Bearer header. The node revokes the token and reports whether a live session was removed.

## Security and lifecycle properties

- Proof of possession is Ed25519 and domain-separated with `AWENET-USER-LOGIN/v1`.
- Challenges and sessions are bounded in memory.
- Challenges are single-use and expire (maximum 300 seconds).
- Sessions expire (maximum 24 hours).
- Only a SHA-256 fingerprint of each bearer token is retained by the session manager.
- Debug output redacts the issued bearer token; its byte array is zeroized when the issued-session value is dropped.
- Account suspension or an account-local sequence change invalidates existing sessions on the next validation.
- Sessions are node-local and memory-only. Restarting the node invalidates all sessions; clients must log in again.

## Current integration boundary

These endpoints establish and validate AWEID sessions. This change does not yet replace the existing signature checks on moderation and network-topology mutation endpoints, nor does it claim that every private API is session-gated. A subsequent change must apply a shared session/capability middleware to the relevant Admin, Node, DC/DG/CG, and Site routes before the session system is considered the complete authorization layer.

The local UI origin/host guard remains in front of these routes. Deployments that expose the UI/API beyond loopback need a separately reviewed TLS and origin policy.
