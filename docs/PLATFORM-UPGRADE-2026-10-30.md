# Node platform services upgrade — 2026-10-30

This upgrade adds a **node platform services layer** to the existing
product stack. It follows the same rules as the observability upgrade
(OBSERVABILITY-UPGRADE-2026-10.md): every module is deterministic and
replay-friendly, holds no wall clock and no threads, keeps hard capacity
bounds, and ships with unit tests. The existing protocol, identity,
routing, storage, messenger and economy modules remain the implementation
building blocks; nothing they export changed.

## New core modules

### alerts.rs — threshold alert rules
- `AlertRule` with Greater/Less threshold operators and severity.
- `AlertEngine::evaluate` turns gauge values into explicit firing-state
  transitions; missing metrics keep the previous state untouched.
- Bounded at `MAX_RULES` (128) with duplicate and validation errors.

### backup.rs — manifests and restore planning
- `BackupManifest` holds content-addressed `BackupItem` entries with
  SHA-256 hex digests, per-item byte counts and kinds (Identity, Storage,
  Names, Store, Messenger, Settings).
- `plan_restore` compares a manifest against locally available digests
  and returns a `RestorePlan` with ready/missing items and a progress
  percentage. Bounded at `MAX_ITEMS` (4096).

### notifications.rs — bounded notification center
- `NotificationCenter` with a hard FIFO capacity bound (default 256),
  stable monotonically increasing ids, read/unread state, per-category
  filtering and full serialization. Reuses `alerts::AlertSeverity` so
  the alert pipeline and the notification center speak the same language.

### quota.rs — accounting and enforcement
- `QuotaTier` limits (storage bytes, transfers per day), `Account`
  usage tracking, and `QuotaDecision` (Allowed/Denied) decisions for
  storage and transfer requests, with overflow-safe accounting and a
  usage-percentage view. Bounded at `MAX_ACCOUNTS` (1024).

### i18n.rs — bilingual product strings
- `Catalog` with English and Armenian (hy) texts, English fallback for
  missing translations, key fallback for unknown keys, and named
  placeholder formatting. `product_catalog()` ships the built-in
  desktop UI strings in both languages.

### update.rs — version contract and upgrade policy
- `Version` parsing, ordering and display for MAJOR.MINOR.PATCH.
- `decide` returns UpToDate, Recommended, Required or Incompatible from
  the current/available versions and the protocol major versions, so node
  upgrades never break protocol compatibility silently.

## Product integration
- The modules are wired into `core/src/lib.rs` and are available to
  `awe-node` and all platform clients through `awep2p_core`.
- The README layer table and documentation index now describe the layer.
- Per the repo status rule, these modules are product plumbing: they are
  covered by unit tests and CI, and end-to-end evidence still follows
  REAL-NETWORK-TEST.md and the readiness gates.
