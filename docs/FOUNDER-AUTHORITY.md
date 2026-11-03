# AWENET Founder Authority — Canonical Specification

**Status:** implementation in progress; this document defines the intended security and product contract.  
**Canonical founder handle:** `AWENeter`  
**Authority model:** one immutable network founder identity, exercised only through explicitly approved founder devices.  
**Scope:** network genesis, identity, authentication, devices, governance, node administration, Neters, ONECOIN policy, recovery, audit, UI, persistence and release acceptance.

This document is normative for the Founder Authority implementation. If code and this specification disagree, do not silently assume either is correct: record the discrepancy, add a regression test, and update the implementation or this document before release.

## 1. Purpose and authority model

AWENET has one founder identity: **AWENeter**. The founder is the highest administrative authority for the network. The founder is not a node, does not have to host a server, rent a VPS, operate a domain, contribute device resources, or keep a particular client online in order to retain founder status.

The founder's authority is a cryptographic capability rooted in the network's pinned Genesis trust record. It is not a username/password role stored in a central database and is not derived from email, phone number, social login, a node ID, device fingerprint, IP address, or membership vote.

The following rules are mandatory:

1. The founder identity and its AWEID are immutable after valid Genesis.
2. The founder cannot be removed, replaced, demoted, or outvoted by members, nodes, Neters, validators, groups, or applications.
3. Community members and AWEMessenger groups may publish advice, objections, proposals and polls. These are advisory unless the founder explicitly adopts a proposal through a founder-signed governance update.
4. Founder authority is exercised only by a cryptographically approved device and valid founder session, subject to emergency locks and operation-specific signatures.
5. A compromised or revoked device must not retain authority merely because it once had a session.
6. The founder does not gain authority over a user's private content simply by being the founder. Private resources remain protected by their own access policy and encryption.
7. The founder may delegate bounded operational tasks to Neters, but delegation never transfers founder status or the ability to change founder rules.
8. No public visitor, first joining node, first account, first server, or first device may silently become founder.

The founder's ability to govern the network is distinct from operating infrastructure. AWENET must continue to function when the founder is offline; queued administrative requests remain pending until an approved founder device can authorize them.

## 2. Authority boundaries

| Actor | May do | Must not do |
| --- | --- | --- |
| Founder | Set network rules; approve/block founder devices; appoint/remove Neters; define scopes and compensation; configure network policy; authorize upgrades and ONECOIN policy changes; lock/unlock founder account or governance | Bypass cryptographic validation; read private content without its resource authorization; silently rewrite signed history |
| Approved founder device | Authenticate as founder and sign only the operations allowed by its active device status and session | Approve itself unless an already-authorized policy explicitly permits that device; survive revocation |
| Neter | Perform explicitly delegated tasks within exact scopes, limits, expiry and audit requirements | Change founder identity/rules, appoint outside its delegation, change compensation, override founder decisions or widen scopes |
| Node operator | Control locally contributed resources and local node safety settings; stop contribution at any time | Claim founder authority or consume resources without consent |
| Member | Own identity, devices, content and participation; submit proposals and tasks | Change founder trust root or force founder election |
| AI assistant/agent | Explain, classify, draft, detect anomalies and recommend actions | Finalize founder-only decisions, appoint Neters, alter compensation, mint/release rewards or override explicit human approval |

A founder action may be executed from any supported client that proves possession of an approved founder-device credential. AWE desktop UI is one client, not the authority itself.

## 3. Cryptographic identity and Genesis

### 3.1 Founder identity

- Reserved handle: `AWENeter`.
- Founder AWEID is derived from the founder's Ed25519 public identity key.
- The founder authority signing key is distinct from the first device authentication key.
- Email and phone may be optional contact/recovery channels in the future, but never define or replace the AWEID.
- The founder's public identity record contains no private key, vault password, bootstrap secret, recovery code, or session token.

### 3.2 Genesis contents

The signed Genesis record binds at minimum:

- format identifier and version;
- unique network ID;
- founder AWEID and founder authority public key;
- initial founder-device public key;
- immutable initial founder rules and device-approval policy;
- creation metadata sufficient for deterministic validation;
- founder signature over a domain-separated canonical payload.

All nodes and clients must validate the same pinned trust root and network ID. A different founder key or network ID is a different network and must not be accepted as an update to the existing network.

### 3.3 One-time initialization

Initialization is an explicit owner-controlled process:

1. Create the founder identity vault for `AWENeter`.
2. Create a separate initial device credential.
3. Generate at least 256 bits of cryptographically random bootstrap secret.
4. Pin the expected domain-separated digest through a trusted local installation/release configuration channel.
5. Initialize Genesis using the founder key, device public key, bootstrap secret and pinned digest.
6. Verify the resulting Genesis and signed state snapshot.
7. Back up the founder recovery material offline and verify the recovery procedure before enabling network administration.

The raw bootstrap secret must never be embedded in browser JavaScript, committed to Git, written to logs, sent to public nodes, or included in diagnostics. The digest is not a replacement for the secret.

Local exclusive file creation protects against a local duplicate initialization race. It does not establish a global election across unrelated nodes. Distributed trust depends on nodes validating the same pinned Genesis.

## 4. Device approval and authentication

### 4.1 Device lifecycle

Device states are: `pending`, `approved`, `denied`, `blocked`, and `revoked`. Transitions must be signed, auditable and bound to the exact device public key, request ID, network ID, decision, sequence and timestamp/tick.

A device request includes a fresh nonce and proof of possession of the requesting private key. Replaying a request or approval must fail.

### 4.2 Approval policies

The founder may choose:

- **All approved devices:** every currently approved, eligible founder device must approve a new device; or
- **Selected devices:** only the exact founder-selected approver set must approve.

The required approver set is snapshotted when the request is created. A later policy change must not silently alter the approvers for an already-pending request. If the policy must change, cancel and reissue the request under the new policy.

If the founder is offline, new-device requests remain pending. There is no timeout-based automatic approval, public fallback, email-only bypass or Neter override.

The last usable approved device cannot be blocked through an ordinary device-management action. Emergency recovery is a separate, stronger, audited process.

### 4.3 Login

A founder login uses a short-lived, unpredictable, single-use challenge bound to:

- network ID and founder AWEID;
- approved device public key and device ID;
- requested session scope;
- challenge nonce and expiry;
- protocol version and domain separator.

The device signs the exact canonical challenge. Validation checks device status, signature, challenge freshness, single-use state, lockout state and emergency account lock. Replayed challenges and signatures must be rejected.

Passwords unlock a local vault; they are not sent to nodes and are not themselves proof of founder authority. Production device keys should use passkeys/WebAuthn, TPM, secure enclave or another OS-protected non-exportable credential where supported. Software-only keys cannot guarantee physical device binding if copied.

### 4.4 Sessions

A founder session is device-bound and has:

- random high-entropy bearer token or equivalent proof-of-possession credential;
- device ID and public-key binding;
- issued-at and expiry;
- exact scopes and risk level;
- revocation/restriction state;
- last-used and audit metadata.

Tokens must never be logged, put in URLs, included in telemetry, or stored unencrypted. Session validation rechecks that the device remains approved and unblocked and that the account is not emergency-locked. Support per-session revoke, revoke-all, expiry, suspicious-session restriction and device-wide invalidation.

High-impact actions (Genesis/trust changes, key rotation, governance rules, emergency unlock, compensation changes and network-wide security policy) require fresh device proof or step-up authentication even if a session is active.

## 5. Founder governance and policy

Founder policy updates are canonical, signed, monotonically sequenced and hash-chained. Each update binds the network ID, founder AWEID, previous update hash, next sequence, operation type, payload hash and protocol version.

Every verifier must reject:

- invalid founder signature;
- wrong network ID or founder AWEID;
- stale sequence, replay, gap or wrong previous hash;
- malformed or non-canonical payload;
- unauthorized policy weakening;
- operation prohibited by either emergency lock.

Immutable founder identity rules cannot be changed through ordinary governance. A future key-rotation process may add successor keys only through an explicit, separately specified, founder-signed transition with dual-key proof and network-wide anti-rollback checks. It must never silently replace the founder AWEID.

Votes, polls and member proposals can inform the founder but cannot replace the founder's signature. The founder may adopt, reject, defer or amend a proposal.

## 6. Neter administration and delegation

Neters are appointed human or service identities that perform bounded operational work.

- Appointment may be made directly by the founder or through an application the founder explicitly approves.
- Every Neter record includes AWEID/public key, status, exact capability scopes, resource/target boundaries, expiry, parent delegator, delegation depth, compensation policy reference and audit metadata.
- Scopes are allow-listed capabilities, not arbitrary role names.
- Delegated scopes must be a strict subset of the parent's scopes.
- Re-delegation is denied unless the founder explicitly grants that capability; a child Neter does not inherit re-delegation automatically.
- Neters cannot change their own scopes, compensation, expiry, status or parent.
- Only the founder can appoint, suspend, reinstate, remove, change compensation, or grant/revoke delegation capability.
- Removal/revocation invalidates active Neter sessions and pending operations immediately.
- Every Neter action is attributed to the Neter and its delegating founder authority chain.

Compensation is a founder-defined policy, not an automatic claim. Any payment must be separately verified against a signed task/contribution receipt and the applicable ONECOIN accounting rules. An AI may recommend a decision but cannot finalize founder-only approval or reward release.

## 7. Emergency locks

Two independent emergency locks are required:

1. **Founder account lock:** prevents new founder login and invalidates or restricts existing founder sessions according to the emergency policy.
2. **Governance lock:** prevents governance changes, privileged policy updates and administrative mutations while allowing read-only status and safety-preserving operations.

Emergency commands are signed by the founder authority key, sequence-bound and audited. Unlocking requires an explicit founder-signed command and stronger re-authentication. A lock must not be bypassable by Neters, ordinary members, node operators, UI routes, API aliases or alternate clients.

Emergency lock behaviour must be defined per operation. For example, node safety shutdown and resource withdrawal must remain available to local operators even when governance is locked.

## 8. Persistence, backup and recovery

Founder state includes:

- Genesis and pinned trust-root metadata;
- approved, pending, denied, blocked and revoked devices;
- governance sequence and hash chain;
- Neter registry, applications, scopes, compensation and audit;
- emergency lock state and signed emergency history;
- security/audit events and anti-replay state.

Persistence requirements:

- bounded, versioned, canonical serialization;
- atomic no-overwrite snapshot publication or a transactional store;
- signature/hash verification on load;
- monotonically increasing sequence and anti-rollback anchor;
- crash-safe fsync and permissions appropriate to the platform;
- encrypted backups for sensitive state;
- restore procedure that never rolls back device revocations or governance sequence;
- corruption detection, backup health and operator-visible recovery status.

Private signing keys, vault passwords, raw bootstrap secrets, recovery codes and session tokens must not be included in public state snapshots. Persisting a snapshot locally is not the same as replicated consensus. Founder login challenges, sessions and failed-login lockout counters are currently volatile process state and are intentionally excluded from signed snapshots. Restart invalidates all sessions and challenges. This is safer than accidentally restoring a bearer session after rollback, but persistent anti-brute-force lockout and durable revocation state are still release requirements. Replicated copies must be authenticated, encrypted where sensitive, and must not be allowed to establish a different trust root.

### Recovery

Recovery must not silently bypass device approval. The planned recovery design uses offline recovery codes and a separately held recovery key, with threshold/dual-proof requirements for high-impact recovery. Recovery must be tested on a clean device and after loss of all ordinary devices. The exact recovery threshold and ceremony must be finalized before production; until then, recovery is explicitly incomplete and must not be simulated.

## 9. Node, API and network integration

The founder is not a node. Founder state may be stored in the founder's own encrypted local vault and distributed as signed public policy/trust records where needed. No always-online founder server is required.

The implementation must separate:

- public read-only network status;
- ordinary member-owned operations;
- node-local operator operations;
- Neter-scoped operations;
- founder-only administrative operations.

Every privileged handler must call a shared authorization layer before reading or mutating protected state. Authorization must verify the signed request/session, founder AWEID, approved device, scope, expiry, current sequence, emergency locks and operation-specific step-up requirements. Route-name checks in the UI are not security.

Privileged routes must fail closed if founder state, trust root, session, device status or audit persistence is unavailable. Public status routes may remain read-only. Local node shutdown and withdrawal of locally contributed resources remain under local operator control.

A signed admin command should bind network ID, actor AWEID, device ID, session ID, command ID, sequence/nonce, operation, canonical payload hash, issued-at and expiry. Each command ID is single-use. API responses must return stable machine-readable error codes and never leak credentials.

The current `GET /api/founder/status` route is read-only observability, not proof of authorization and not a complete control API. The current runtime snapshot type is a persistence foundation; callers must persist every successful mutation. No operation should be described as founder-protected until the enforcement middleware is actually on its execution path and has integration tests.

### Current founder login HTTP contract

These endpoints are served by the local node UI listener. They are not a remote administration transport and should remain loopback-only until an authenticated encrypted remote control protocol is implemented. The API does not emit wildcard CORS permissions.

Request a challenge for a device already present in the approved device registry:

```http
POST /api/founder/login/challenge
Content-Type: application/json

{"device_id":"<64 lowercase hex characters>"}
```

A successful response contains `challenge_id`, `device_id`, `nonce`, `issued_tick`, `expires_tick`, `message` and `algorithm: "Ed25519"`. `message` is the hex encoding of the exact canonical byte string returned by the core `login_message` function. The client must hex-decode those bytes and sign them with the approved device private key. It must not reconstruct or alter the statement.

Complete the challenge:

```http
POST /api/founder/login/complete
Content-Type: application/json

{"challenge_id":"<64 hex>","signature":"<128 hex>"}
```

The response contains a 32-byte bearer token, device ID and expiry. The token is shown only once and must be kept in volatile client memory; never put it in a URL, logs, telemetry or persistent browser storage.

Validate a session:

```http
GET /api/founder/session
Authorization: Bearer <64 hex token>
```

Revoke it:

```http
POST /api/founder/logout
Authorization: Bearer <64 hex token>
```

Challenge and session state is currently process-memory-only. Restart invalidates all active sessions and pending login challenges. This is an explicit temporary boundary, not durable session support. Failed-login counters are also currently volatile; persistent rate-limit state is required before production. These endpoints authenticate a founder device but do not yet authorize Neter, governance, network-policy or ONECOIN mutations.


## 10. Founder Control Center UX

The Control Center is a first-class area in the AWENET product, available only after local vault unlock and approved-device authentication. It must work on desktop, tablet and mobile, be Armenian-first with English support, accessible, responsive, and usable with keyboard and screen readers.

Required sections:

1. **Overview:** founder identity/AWEID, network ID, trust-root health, current device, active sessions, pending approvals, active Neters, audit alerts and network status.
2. **Devices:** pending requests, approver policy, device fingerprints/public-key IDs, approve/deny, block/unblock, revoke and session invalidation.
3. **Security:** login history, failed attempts, active sessions, suspicious activity, key status, backup status and emergency locks.
4. **Governance:** current rules, immutable rules clearly labelled, signed policy history, proposals and approval/rejection workflow.
5. **Neters:** applications, appointments, scopes, delegation tree, status, compensation policy, expiry and audit.
6. **Network administration:** versioned network policy, node admission and service configuration, with clear separation from local node-owner controls.
7. **ONECOIN policy:** reward policy, task authorization, verifier configuration, economic policy proposals and audit; no UI action may bypass ledger/consensus verification.
8. **Recovery:** backup verification, recovery-key status, recovery instructions and explicit warning when recovery is not configured.
9. **Audit:** append-only searchable history, filters, export, signature verification and integrity warnings.

Destructive or high-impact actions require clear previews, exact target confirmation, step-up authentication and an audit record. Never use deceptive success messages or pretend a remote action completed before the node confirms it.

## 11. Audit, privacy and observability

Audit events must be append-only, bounded/archived without losing verifiability, hash-linked or signed, and include actor, device, operation, target, outcome, reason code, sequence and time. Sensitive payloads are represented by hashes or redacted fields.

Never log passwords, private keys, bootstrap secrets, recovery codes, session tokens, raw authentication challenges or private message/file content. Audit export must be verifiable and must distinguish local observations from network-confirmed events.

The founder's administrative authority does not grant blanket visibility into private user content. Access to private data follows the resource owner's access descriptor, encryption and explicit authorization rules.

## 12. Threat model and limitations

The design must address stolen devices, copied software keys, malicious clients, replay, brute force, compromised nodes, stale backups, rollback, corrupted snapshots, malicious Neters, malicious insiders, network partitions, delayed approval delivery, and denial of service.

Security controls reduce risk; they do not make attacks impossible. A modified client can discard a trust root, and a compromised endpoint can misuse keys while unlocked. Independent nodes can disagree during partitions; local persistence does not create distributed consensus. A public server cannot be assumed honest solely because it is popular or federated.

The network must fail closed for founder-only operations when authorization cannot be verified, while preserving safe local operator controls and ordinary user access where independent policy allows it.

## 13. Implementation status

### Implemented in the current branch (must remain subject to CI verification)

- Signed one-time Genesis and bootstrap-digest validation.
- Reserved founder identity and distinct founder/device key roles.
- Device proof-of-possession and all-approved/selected-device approval policies.
- Signed device decisions and controls, including last-approved-device protection.
- Device-bound login challenges, replay protection, lockout, expiring/revocable/restrictable sessions and global logout.
- Signed sequence/hash-chained founder governance updates.
- Neter applications, appointments, scoped delegation and founder-controlled compensation/status.
- Two signed emergency-lock levels and audit records.
- Persistent founder runtime snapshots with signature verification and damaged-latest fallback.
- Explicit local founder initialization/status/digest CLI commands.
- Node startup loads founder state; read-only `GET /api/founder/status` exposes status.
- Device-signed login endpoints are present: `POST /api/founder/login/challenge`, `POST /api/founder/login/complete`, `GET /api/founder/session`, and `POST /api/founder/logout`. Challenges and sessions currently live in process memory and are invalidated by restart; durable transactional session/revocation storage remains required.
- The desktop UI has a read-only Founder Authority overview that consumes the live status endpoint. It does not pretend to provide the full administration console or device-key signing bridge.

### Not yet complete — do not claim production readiness

- Full first-run and login UI / Founder Control Center (a read-only overview is present; secure device signing and administrative workflows are not).
- Secure UI-to-core signing bridge and OS-backed non-exportable key integration.
- Remote device-approval delivery and pending-request workflow.
- Durable transactional session/revocation storage; current login challenges and sessions are intentionally memory-only and are lost on restart.
- Recovery ceremony, recovery UI and clean-device recovery tests.
- Durable transaction API that automatically persists every mutation and safely handles concurrent writers.
- Shared authorization middleware enforced on every privileged API, node admin action, Neter action, governance mutation and ONECOIN policy endpoint.
- Replicated trust-root distribution, anti-rollback anchor and independent-node verification.
- Complete append-only durable audit export and backup/restore.
- Full Linux/Windows workspace, product smoke, Clippy, security audit and end-to-end tests on the final commit.

## 14. Release acceptance checklist

A release is not complete until all applicable checks pass with evidence:

- [ ] Fresh Genesis can be initialized once; concurrent and second initialization fail safely.
- [ ] Wrong bootstrap secret, wrong founder key, wrong network ID and altered Genesis are rejected.
- [ ] Founder authority key and initial device key are distinct and correctly bound.
- [ ] All-approved and selected-device policies work, including offline approvers and pending requests.
- [ ] Denial, block, revoke and last-device protections work; revocation invalidates sessions immediately.
- [ ] Challenge expiry, replay, brute-force lockout, session expiry, global logout and suspicious-session restriction work.
- [ ] Governance signatures, sequence, previous-hash, anti-replay and immutable-rule checks work across independent nodes.
- [ ] Neter scopes cannot widen through direct or transitive delegation; compensation/status changes are founder-only.
- [ ] Both emergency locks block exactly the required operations and cannot be bypassed through alternate routes.
- [ ] Every privileged API and background job is protected by shared middleware and integration-tested.
- [ ] Every successful state mutation is crash-safe, durable and anti-rollback protected.
- [ ] Backup restore does not resurrect revoked devices, sessions or old governance state.
- [ ] UI actions require real core authorization and report confirmed outcomes only.
- [ ] Private data, secrets and session tokens do not leak through logs, API responses or audit exports.
- [ ] Clean-machine Windows and Linux install, login, device-loss recovery and upgrade tests pass.
- [ ] Independent nodes accept the same trust root and reject conflicting Genesis/policy histories.
- [ ] Formatting, workspace tests, Clippy, security audit and release smoke tests pass on the exact release commit.

## 15. Development order

1. Preserve the current Genesis/device/auth/admin domain core and finish CI verification.
2. Introduce a single transactional FounderStateStore that persists every mutation and protects against concurrent writers and rollback.
3. Build the shared founder authorization/command-verification middleware and cover every privileged execution path.
4. Implement login, device approval and recovery protocols with signed request/response messages.
5. Build the Control Center UI on those real APIs; do not build fake demo actions.
6. Add OS credential integration and independent-node trust-root/anti-rollback distribution.
7. Run adversarial, cross-platform, recovery and release acceptance tests.
8. Only then mark the PR ready and merge through the repository's normal review process.

**Completion definition:** Founder Authority is complete only when the founder can securely authenticate from an approved device, manage the network through real protected operations, recover safely, delegate bounded tasks, inspect verifiable audit history, and retain immutable founder authority without operating a server — and every security and release acceptance check above has passed.
