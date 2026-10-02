# AWEP2P Product Readiness Gates

This document turns the project's engineering charter into release gates. It is intentionally evidence-based: a module is not considered production-ready because its types compile or its UI displays a success state.

## Release levels

- **Prototype:** local behavior works; network, persistence, or security limitations are documented.
- **Preview:** reproducible builds and automated tests pass; failures are surfaced to the user; data formats are versioned.
- **Production candidate:** end-to-end tests cover real peers and persistent state; upgrade, recovery, resource-limit, and adversarial-input behavior are tested.
- **Production:** supported platform packages are reproducibly built, signed, documented, and have a tested update and rollback path.

A feature must use the lowest level whose evidence is available. Do not label the whole ecosystem production-ready while any critical dependency remains a stub.

## Critical end-to-end gates

### Node lifecycle
- [ ] First launch creates or imports an identity without requiring an email, phone number, or central account.
- [ ] Node starts, reports its actual listen address and capabilities, and shuts down cleanly.
- [ ] Restart preserves identity and durable state; corrupt state produces a recoverable error rather than silent reset.
- [ ] Resource limits (disk, memory, bandwidth, CPU, battery) are enforced at the operation boundary.

### Peer connectivity and routing
- [ ] Two independently started nodes authenticate each other over real sockets.
- [ ] A request traverses a verified path; every hop forwards the same request with bounded hop count and replay protection.
- [ ] Route selection uses observed peer state and fails explicitly when no route exists.
- [ ] Disconnects, stale peers, malformed frames, and timeouts are covered by integration tests.
- [ ] NAT traversal and relay are marked unavailable unless a real protocol exchange and connectivity test succeed.

### Distributed storage
- [ ] Upload encrypts client-side before sending private content to another node.
- [ ] Placement uses only authenticated, online nodes with available capacity.
- [ ] Each accepted shard is persisted and acknowledged only after integrity verification and durable write.
- [ ] Download retrieves shards from remote peers, verifies each shard, and reconstructs the original bytes.
- [ ] Interrupted transfers can resume without corrupting existing data.
- [ ] Repair moves or regenerates actual shard bytes, verifies the destination write, and updates durable metadata only after success.
- [ ] Health reports distinguish local copies, reachable remote copies, missing copies, and unknown status.

### Names, hosting, and applications
- [ ] A name resolves from a verified registry record, with sequence/freshness and signature checks.
- [ ] A site is retrieved from a remote node using a versioned manifest and content hashes.
- [ ] Store packages are signature-verified before installation; declared capabilities are enforced at runtime.
- [ ] WASM execution is only reported when a real runtime executes the module under enforced limits.
- [ ] Messenger delivery status reflects remote acknowledgement, not merely local queue insertion.

### Product and release quality
- [ ] Desktop UI launches without a terminal and explains node state, errors, and resource usage.
- [ ] CLI and UI use the same core operations and report consistent results.
- [ ] Windows and Linux builds run in CI; Android builds are separately validated on supported toolchains/devices.
- [ ] Release artifacts include version, platform, checksum, and signing information.
- [ ] Upgrade, rollback, backup, and identity recovery procedures are documented and tested.
- [ ] No secrets, private content, or unnecessary personal data appear in logs.

## Current known blockers to close

The following items must not be represented as completed until their end-to-end gates above pass:

1. **Repair execution:** planning a replacement is not repair. A successful result requires a real shard copy/reconstruction, destination persistence, integrity verification, and metadata update.
2. **Multi-hop routing:** a ranked list of peers is not a route. Each hop must be connected to the next, forward a bounded request, and return a correlated response.
3. **STUN / hole punching:** a packet data structure is not NAT traversal. A real STUN exchange, mapped-address handling, peer coordination, and connectivity test are required.
4. **Distributed storage:** local content-addressed storage and placement calculations do not alone prove remote upload/download, durability, or recovery.
5. **Product packaging:** a binary existing in the repository does not prove that it is current, reproducible, signed, or runnable on a clean machine.

These are implementation requirements, not claims that the features are impossible. Keep the user-facing status precise while closing them.

## Pull request acceptance

Before merging a networking or storage change:

1. Run formatting, workspace check, all tests, and Clippy with warnings denied.
2. Require CI results for the exact head commit; an absent status is not a passing status.
3. Review the diff for fake success paths, unbounded work, unsafe persistence ordering, and accidental secret logging.
4. Include at least one test that exercises the actual boundary changed (socket, filesystem, cryptographic verification, or process launch), not only a planner or struct.
5. Update the daily status with what was actually implemented and what remains.

A PR may be technically mergeable while still failing these product acceptance gates.
