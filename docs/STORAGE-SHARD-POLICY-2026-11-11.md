# Storage shard policy consistency

## Problem

`StoragePolicy::for_file_size()` scales the erasure-coding shard count with the
object size: small files use the 12-shard floor, and the count grows by 12 for
every 4 MiB chunk group until it saturates at the 1000-shard network ceiling.

The node HTTP API did not follow that rule. Three endpoints reported or assumed
the ceiling unconditionally:

| Endpoint | Old behaviour | Impact |
| --- | --- | --- |
| `POST /api/storage/put` | response body hardcoded `"shards": 1000` | clients were told a 12-shard object had 1000 shards |
| `POST /api/storage/push` | `total_shards` hardcoded to `1000` in the shard transfer, and `shard_index < 1000` accepted | every pushed shard of a small file failed `StorageShardTransfer::verify()` (`shard_index out of range`), so remote replication of small objects silently never completed |
| `GET /api/storage` | `replication_policy.shards` hardcoded to `1000` | the reported policy disagreed with the policy actually used for new uploads |
| `GET /api/storage/get` | missing `shards` in a manifest defaulted to `1000` | older manifests allocated 1000 shard slots and could not reconstruct |

## Fix

* `/api/storage/put` now reports `shards`, `data_shards`, `parity_shards` and
  `replicas` from the same `StoragePolicy::for_file_size(data.len())` value that
  encoded the shards.
* `/api/storage/push` derives the policy from the request's `original_size`,
  validates `shard_index` against that policy, and sends the matching
  `total_shards` in the transfer. The error message reports the real upper bound.
* `/api/storage` reports the default policy shard count plus an explicit
  `max_shards: 1000` ceiling, so the ceiling is still discoverable without
  pretending it is the per-object value.
* `/api/storage/get` falls back to `StoragePolicy::for_file_size(original_size)`
  when a manifest has no `shards` field.

## Tests

* `core/src/storage.rs`
  * `for_file_size_scales_shards_with_file_size` — 12 at the floor, 24 past the
    first 4 MiB group, 1000 at saturation.
  * `for_file_size_keeps_data_and_parity_consistent` — data + parity always
    equals the total, replicas stay at 3.
  * `small_file_policy_roundtrips_through_erasure_coding` — a 2 KiB object
    encodes to 12 shards and still recovers after two shards are lost.
* `core/src/data_plane.rs`
  * `small_file_transfer_verifies_with_scaled_policy` — a 12-shard transfer
    verifies, the last index is accepted, the next index is rejected.
* `apps/awe-node/tests/product_smoke.rs`
  * the three-node smoke test asserts that a tiny upload reports `"shards":12`
    and never `"shards":1000`.

## Invariants

1. The shard count reported by an endpoint must equal the count used to encode
   the object.
2. `shard_index` must be `< total_shards` for the object's own policy.
3. `data_shards + parity_shards == total_shards` and `12 <= total_shards <= 1000`.
4. `replica_count` stays 3 for every scaled policy.
