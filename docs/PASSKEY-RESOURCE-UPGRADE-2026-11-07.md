# Founder passkey ceremony & contributed storage metering (2026-11-07)

This upgrade closes three of the honest gaps left open by the [2026-11-06
product layer](PRODUCT-UI-UPGRADE-2026-11-06.md) without touching any
existing protocol guarantee, data format or screen.

## 1. Browser WebAuthn passkey ceremony

`core/src/webauthn.rs` (new) implements the backend half of the WebAuthn
ceremony: a strict CBOR decoder, authenticator-data and COSE-key parsing,
and Ed25519 (COSE alg -8) assertion verification through the existing
ed25519-dalek identity stack. `core/src/founder_guard.rs` gains raw-key
enrollment (`enroll_passkey_bytes`) and assertion-based step-up completion
(`complete_step_up_assertion`).

The node economy API exposes the ceremony to the desktop UI, which is
served on `http://localhost:41800`:

| Endpoint | What it does |
| --- | --- |
| `GET /api/founder/passkey/status` | Enrolled passkeys, MFA session state, remaining recovery codes |
| `POST /api/founder/passkey/register/options` | Opens a registration ceremony with a server-side challenge |
| `POST /api/founder/passkey/register` | Verifies the attestation and enrolls the credential (needs a live MFA session once a passkey already exists) |
| `POST /api/founder/passkey/step-up/options` | Opens an MFA step-up ceremony (requires one enrolled passkey) |
| `POST /api/founder/passkey/step-up` | Verifies the assertion and opens a bounded MFA session |

Security properties:

- The ceremony challenge lives server-side, is bound into `clientDataJSON`
  and expires after 10 minutes; replaying an old ceremony cannot satisfy a
  fresh endpoint call.
- The credential must be bound to the `localhost` relying party and be an
  Ed25519 (OKP, alg -8) COSE key; ES256 credentials are refused.
- Enrollment is authorized by the founder signature over the credential
  public key, computed on the founder machine; it never travels the wire.
- Once at least one passkey is enrolled, every mutating founder endpoint
  (publish task, price changes, approve/reject/revise submissions, team
  payouts, close/cancel tasks, approve suggestions, resolve disputes,
  founder refunds) requires a live MFA step-up session. The gate is
  enforced in the backend, not in the UI.
- Step-up challenges, open MFA sessions and browser ceremonies are
  in-memory auth state: the durable economy snapshot stores only the
  enrolled credentials and counters, and a restart requires a fresh
  step-up.
- A failed assertion consumes the pending challenge; recovery codes still
  replace the whole credential set exactly once, as before.

## 2. Contributed storage metering

`core/src/resource_meter.rs` now reads the free capacity of the
contributed storage directory via `statvfs` (Linux) and integrates it into
`storage_gb_hours` exactly like the other counters: the per-period value
is the average available GB across two reads multiplied by the elapsed
time. Nothing is paid for declared-but-unmeasured space, and the resource
economy's grant limits still cap what settles.

## 3. Camera QR scan-to-pay

`apps/awe-desktop/ui/onecoin.js` adds a Scan to pay button next to
Receive. Where the browser exposes `BarcodeDetector` and camera access,
it reads a recipient AWE ID QR and fills the transfer form; otherwise it
says so honestly and the AWE ID can still be pasted. Browsers without the
API get a clear message instead of a broken button.

## Honest not-implemented list

- Attestation trust chains (attStmt signatures, certificate hierarchies)
  are not validated; security rests on challenge binding, the
  relying-party hash check and the founder-signed enrollment.
- ES256 and other credential algorithms are refused; Ed25519 only.
- Multi-node resource verifiers, multi-node marketplace sync and GPU
  metering remain as in the 2026-11-06 doc.
- Economy screens remain English-only; localization is still pending.
- The ceremony runs on the node UI origin; there is no remote
  (non-loopback) founder console.
