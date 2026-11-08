# Economy UI localization (Armenian + English) — 2026-11-08

This upgrade closes the economy-screen localization gap listed in
[PRODUCT-UI-UPGRADE-2026-11-06.md](PRODUCT-UI-UPGRADE-2026-11-06.md) and
[PASSKEY-RESOURCE-UPGRADE-2026-11-07.md](PASSKEY-RESOURCE-UPGRADE-2026-11-07.md):
every economy screen in the AWENET desktop product now renders in both
English and Armenian, from a single bilingual catalog.

## What was added

### One catalog as the source of truth

- `core/src/i18n.rs` grows from 22 to 400 keys. The existing product keys
  are unchanged; the economy screens add `awk_*` (AWENETwork board, task
  detail, submissions, profile, suggestions), `mkt_*` (storefront catalog,
  seller page, orders, escrow), `fnd_*` (Founder Control Center: overview,
  publish, review, price, suggestions, store, passkey security) and `oc_*`
  (ONECOIN wallet, exchange, rewards, tiers), plus shared `status_*`,
  `cat_*`, `cx_*`, `kind_*`, `level_*` and `tier_*` families.
- New API on the same module: `Locale::from_code` (strict `en`/`hy`
  parsing) and `Catalog::messages(locale)` returning the flat key-to-text
  map with the same fallback rules as `Catalog::get` (Armenian falls back
  to English only when a translation is empty; unknown keys are omitted).

### The node serves the catalog

- `GET /api/i18n/messages?locale=en|hy` in `apps/awe-node/src/economy_api.rs`
  returns `{status, locale, fallback_locale, message_count, messages}`.
  An unknown locale is rejected with 400.
- `main.rs` serves `ui/i18n.js` at `/i18n.js` and routes `/api/i18n/*`
  through the economy dispatcher (loopback origin guard unchanged).

### The desktop UI loads and switches languages

- New `ui/i18n.js` exposes `window.AWEI18N` with `t(key, args)` (named
  `{placeholder}` substitution, key fallback), `ready()` (catalog fetch),
  `locale()` and `setLocale()`. The chosen locale persists in
  `localStorage["awenet.locale"]`; `setLocale` refreshes the topbar toggle
  and re-renders the active screen by re-clicking the active nav entry.
- `index.html` adds the topbar `ՀԱ/EN` language toggle and loads
  `i18n.js` before the economy screens.
- `awenetwork.js`, `storefront.js`, `founder.js` and `onecoin.js` are
  rewritten to render every user-visible string through `AWEI18N.t`,
  awaiting `AWEI18N.ready()` before the first render. Logic, API calls and
  event wiring are unchanged; backend status/category/kind identifiers are
  mapped to translated labels at render time.

## What is deliberately not changed

- No economy rule, permission, pricing or payout behaviour changed. The
  founder-only backend gates are untouched; localization is presentation
  only.
- Brand names (AWENET, AWENETwork, ONECOIN, ONEBANK, AWE ID) are not
  translated.
- The official-price disclaimer and all economy disclaimers stay attached
  to the screens exactly as before.

## Honest status

- Implemented and tested in this upgrade: the bilingual catalog, the
  messages endpoint, the loader, the toggle, and full en/hy rendering of
  the AWENETwork board, task detail, work profile, suggestions, storefront
  (browse/sell/orders), Founder Control Center (all seven tabs) and the
  ONECOIN wallet screen (wallet, exchange, rewards, tiers).
- Not localized yet: the non-economy desktop screens (Home, Browser,
  Messenger, Storage, Store app, Settings, Developer) still render English
  literals. The catalog, endpoint and loader now exist, so extending them
  is additive key-by-key work; each screen keeps its current behaviour
  until its keys and tests land.
- Still open from previous upgrades (unchanged): multi-node usage
  verifiers, GPU metering, multi-node marketplace sync.

## Tests

- `core` unit tests: `parses_locale_codes`,
  `economy_keys_exist_in_both_locales` (catalog size and spot keys),
  `every_catalog_entry_has_english_and_armenian_text` (all 400 entries have
  both texts), `messages_map_omits_unknown_keys`.
- `apps/awe-node/tests/product_smoke.rs` now asserts against a running
  node: the Armenian catalog response contains `"status":"ok"` and Armenian
  text, the English catalog contains the English economy strings, an
  unknown locale returns a 400 JSON error, and `/i18n.js` is served with
  the `AWEI18N` loader.
- The four screens are syntax-checked JavaScript with no user-facing
  English literals outside brand names and raw data values.
