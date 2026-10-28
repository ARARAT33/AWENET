# Contributing to AWENET

AWENET is the decentralized networking and resource layer of the AWE
ecosystem. Contributions are welcome — code, docs, testing on real
hardware, and issue reports all move the network forward.

## Ground rules

1. **CI is the gate.** Every pull request must pass:
   ```bash
   cargo fmt --all -- --check
   cargo test --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   ```
2. **No unfinished work ships.** The architecture job rejects markers such as
   `TODO`, `FIXME`, `UNIMPLEMENTED` and similar placeholders in
   `core`, `apps` and `platforms`.
3. **A screen is not a proof.** Distributed features additionally need
   evidence from independent nodes — see
   [docs/REAL-NETWORK-TEST.md](docs/REAL-NETWORK-TEST.md).
4. **Secrets stay local.** Never commit vaults, `.awesecret` files, keys or
   private content.

## Workflow

1. Branch from `main` using `feat/`, `fix/` or `docs/` prefixes.
2. Keep pull requests small and focused; one PR per logical change.
3. Describe what changed and how you verified it (commands, test output,
   node setup for distributed checks).
4. Allow maintainers to push fixes to your branch when possible.

## Desktop UI work

The desktop UI lives in `apps/awe-desktop/ui` as plain HTML, CSS and
JavaScript that the Rust node embeds at compile time. Keep it
dependency-free, escape user-supplied values before rendering, and verify
every screen against a running local node.

## Reporting issues

Use the issue templates (bug report or feature request). For security
matters, describe the impact without publishing exploit details.
