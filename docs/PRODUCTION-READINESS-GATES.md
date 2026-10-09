# AWENET Production Readiness Gates

This checklist is an evidence gate, not a claim that the product already passes. Mark a gate **PASS** only when its test command, environment, commit, and output are recorded. A source module, mock, local-only test, or UI screen alone is not proof of end-to-end behavior.

## Release rule

Do not label a build production-ready while any critical gate is unverified or failing. Keep implementation work separate from release verification, but run the full verification suite before publishing a release.

## P0 — Build, startup, and data safety

- [ ] Clean checkout builds in locked/reproducible mode where supported.
- [ ] Formatting, workspace tests, and Clippy pass on the supported Rust toolchain.
- [ ] Node starts without a browser tab being required for the desktop application.
- [ ] Fresh install, upgrade, restart, and uninstall are exercised on each supported OS.
- [ ] Vault creation, unlock, wrong-password rejection, backup/export, and recovery are tested.
- [ ] Secrets, private keys, passwords, and private content never appear in logs or diagnostics.
- [ ] Disk-full, permission-denied, corrupted-state, and interrupted-write cases fail safely.

## P0 — Network and identity

- [ ] Two independent processes on separate machines establish an authenticated session.
- [ ] Invalid identity/signature, malformed frames, replayed messages, and unsupported protocol versions are rejected.
- [ ] Reconnect, peer loss, duplicate connection, timeout, and abrupt shutdown are tested.
- [ ] Firewall restrictions and unreachable peers produce actionable errors without hanging.
- [ ] Privacy claims are tested against observed metadata. Encryption must not be described as anonymity.

## P0 — Distributed storage

- [ ] Upload from node A and retrieve from node B after the upload session ends.
- [ ] Retrieved bytes are verified against the content identifier and manifest.
- [ ] Replicas are counted by distinct, confirmed storage nodes—not merely requested placements.
- [ ] A replica on an offline or unhealthy node is not reported as healthy.
- [ ] Node loss triggers bounded repair and does not create unbounded retry traffic.
- [ ] Partial upload, corrupted chunks, insufficient capacity, and interrupted downloads are handled safely.
- [ ] Reserved storage and contribution limits are enforced before accepting work.

## P1 — Product flows

- [ ] Open and secret objects have distinct, tested access rules; secret objects require the authorized map/capability.
- [ ] Mutable display names do not change immutable content identity.
- [ ] Site/namespace resolution works from a second independent node.
- [ ] Messenger delivery, group membership, notifications, and reconnect behavior work across nodes.
- [ ] Store package identity, publisher authenticity, permissions, and update integrity are verified.
- [ ] ONECOIN balances and transfers are tested for replay, duplicate submission, invalid signatures, and concurrent updates before any real-value claim.

## P1 — User experience and platform packaging

- [ ] Every visible button is connected to a real operation or clearly marked unavailable.
- [ ] Loading, empty, success, permission-denied, offline, and failure states are present.
- [ ] UI actions surface useful errors instead of generic network failures.
- [ ] English is consistent across all shipped screens and dialogs.
- [ ] Windows, Linux, and Android artifacts install and launch on clean supported devices.
- [ ] UI never reports success before the underlying operation succeeds.

## P2 — Scale and operations

- [ ] Resource quotas bound CPU, memory, bandwidth, disk usage, and concurrent requests.
- [ ] Load tests cover increasing peer counts, large files, churn, and slow peers.
- [ ] Backpressure and rate limits protect nodes from abusive or overloaded peers.
- [ ] Diagnostics expose health and counters without exposing private data.
- [ ] Release artifacts have checksums, version information, and documented rollback/recovery steps.

## Evidence record

For each test, record:

| Field | Required evidence |
|---|---|
| Commit | Exact Git SHA |
| Environment | OS, architecture, Rust/toolchain version |
| Topology | Number of independent nodes and network conditions |
| Command / scenario | Exact reproducible steps |
| Expected result | Explicit pass condition |
| Actual result | Unedited output with secrets removed |
| Status | PASS, FAIL, BLOCKED, or NOT RUN |
| Follow-up | Bug link and owner/next action |

## Current status

All boxes are intentionally unchecked until fresh evidence is attached. This document does not assert that any gate has passed. Begin with P0 failures, fix the underlying implementation, then repeat the affected tests and the complete release suite.
