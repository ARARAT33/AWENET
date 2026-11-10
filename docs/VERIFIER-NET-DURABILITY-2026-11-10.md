# Verifier network durability and validation — 2026-11-10

## Scope

This increment hardens the core::verifier_net state introduced on
2026-11-10. It closes a trust-boundary gap between the serializable
in-memory model and any future disk/network restore path.

## Changes

- VerifierRegistry::is_active now requires the certificate's founder
  signature and verifies that the certified AWE ID is derived from the
  public key. A forged certificate inserted by deserialization cannot
  make an untrusted verifier active.
- Registry validation checks certificate signatures, AWE ID/public-key
  binding, duplicate public keys, duplicate/zero/future sequence values,
  registry bounds, and retirement timestamps.
- Enroll and retire use checked sequence increments. Exhaustion fails
  before mutating the registry instead of saturating and reusing the
  same sequence.
- Attestation-window deadline arithmetic is checked; a period whose
  deadline would overflow is rejected without mutation.
- VerifierNet::validate checks the full registry, window keys and
  bounds, deadline consistency, report/window binding, report
  signatures, certified verifier keys, self-attestation and duplicate
  signers.
- Consensus construction fails closed when the restored state is
  inconsistent and deduplicates signer keys before adding reports.
- save and load add a founder-signed snapshot envelope. The store
  uses a versioned domain-separated signature, a 64 MiB read/write
  bound, OS-CSPRNG temporary names, create_new, Unix mode 0600,
  write_all + sync_all, atomic rename, cleanup on failure and
  best-effort parent-directory sync.
- The founder private key is not serialized. Loading requires the
  already-installed founder identity and verifies both the outer
  snapshot signature and every inner registry/report invariant.

## Regression coverage

The added tests cover:

1. forged certificates cannot become active or submit reports;
2. sequence exhaustion on enroll/retire does not mutate state;
3. verifier re-enrollment cannot move the retirement timestamp backwards;
4. deadline overflow is rejected without creating a window;
5. founder-signed snapshot save/load round-trip and wrong-founder rejection;
6. an outer founder signature cannot bless an internally forged verifier
   certificate;
7. an invalid save leaves the previously committed snapshot unchanged.

The existing Serde round-trip test now also runs full state validation.

## Verification boundary

This is a core durability primitive, not yet a node-runtime integration.
It does not add HTTP routes, A2P2 transport messages, peer-to-peer
snapshot replication, conflict resolution, or automatic startup loading
in awe-node. Those remain required before verifier enrollment and
attestation state are live network services.

No local Cargo result is claimed. The PR's exact-head Linux and Windows
workspace build, unit tests, product smoke, Clippy, format and audit
must pass before merge.
