# AWEP2P Engineering Charter

This charter defines how the repository moves from an experimental Rust foundation to a dependable, installable end-user product.

## Scope

Review the repository and its current behavior before each development cycle. Changes must be focused, reviewable, tested, and documented. Do not infer feature completeness from filenames, UI controls, structs, comments, or old status documents.

## Non-negotiable rules

1. **No fake success:** report success only after the operation has actually completed and its result is verified.
2. **No fabricated topology:** peer identity, availability, capacity, health, and routes must be based on observed or cryptographically verified state.
3. **Evidence-based security:** do not promise perfect security, invisibility, or absolute anonymity.
4. **Data minimization:** no mandatory email, phone number, real name, or central account for core identity.
5. **Secret protection:** keep private keys and credentials protected at rest; never log secrets. Plaintext export must be explicit.
6. **Verify at boundaries:** validate signatures, hashes, lengths, versions, sequence numbers, permissions, and replay state where untrusted data enters.
7. **Durability honesty:** local storage is local storage. Do not call it distributed or durable until remote persistence and recovery are tested.
8. **Resource safety:** bound packet sizes, queues, lookup fanout, retries, disk use, and CPU/memory work.
9. **Test the changed boundary:** networking changes need socket-level tests; persistence changes need filesystem/restart tests; packaging changes need clean-install tests.
10. **CI before merge:** require successful checks for the exact PR head commit. Missing checks are not green.
11. **Incremental delivery:** prefer one end-to-end vertical slice over many disconnected placeholders.
12. **Documentation consistency:** update the relevant README, status, and readiness notes whenever behavior or maturity changes.

## Product maturity levels

- **Prototype:** a local component or experiment; limitations are explicit.
- **Preview:** reproducible build and automated component tests; user-visible failures are honest.
- **Production candidate:** independent-node integration, persistence, recovery, resource-limit, and adversarial-input tests pass for the supported scope.
- **Production:** signed/reproducible packages, clean-machine installation, update/rollback, operational diagnostics, and documented support boundaries are validated.

Always describe a feature using the lowest level supported by evidence. The whole ecosystem cannot be called production-ready while a critical dependency remains experimental.

## Product architecture

The intended ecosystem includes Node, Drive, Host/Sites, Store, Messenger, AWEwww/AWETLD/AWEOpen, and platform clients. Shared protocol and core behavior should remain independent of UI. The UI is part of the product and must expose real operations, states, permissions, errors, and resource usage.

## Development workflow

1. Inspect current `main`, in-scope files, recent commits, and CI.
2. Select a concrete blocker with a clear user-visible outcome.
3. Implement the smallest complete vertical slice.
4. Add tests at the actual changed boundary and update documentation.
5. Run formatting, workspace checks, tests, and Clippy where applicable.
6. Open a PR against `main`; include scope, evidence, limitations, and test results.
7. Merge only when required checks are green and the diff is coherent.
8. Update the daily status with implemented behavior and remaining work.

## Completion evidence

A feature is complete only when its acceptance criteria are reproducible, its failure paths are tested, and its UI/CLI status reflects reality. A successful compile is necessary but not sufficient.
