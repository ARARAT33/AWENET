# Product Health Score

The product health score (`core/src/product_health.rs`) answers one operator
question quickly: **is my AWEp2P network healthy right now, and if not, what
should I fix first?**

It complements `core/src/diagnostics.rs`, which tracks a *single node*.
The health score aggregates the *whole product view*.

## Inputs

| Factor | Meaning | Default weight |
| --- | --- | --- |
| `replica_durability` | Healthy replicas as % of the replication target | 40 |
| `peer_coverage` | Healthy peers as % of all known peers | 25 |
| `transfer_success` | Successful transfers as % of finished transfers | 20 |
| `dht_responsiveness` | DHT responsiveness percentage | 15 |

Weights are configurable (`HealthWeights`) and must sum to 100. Every input
percentage must also be in the inclusive `0..=100` range. Invalid weights or
out-of-range inputs make `compute_report` return `None` instead of producing a
nonsense score.

## Runtime integration contract

`product_health` is deliberately a pure scoring and history module. Runtime
adapters must supply each factor from observed evidence; they must not replace
unknown data with a healthy default. A node integration should therefore:

1. calculate replica durability from verified durable replica acknowledgements;
2. calculate peer coverage from authenticated peers and fresh liveness data;
3. calculate transfer success from completed transfer outcomes; and
4. calculate DHT responsiveness from bounded discovery/routing probes.

Until all four evidence sources are available, the score must be presented as
incomplete or clearly marked as partial rather than advertised as production
readiness.

## Score and bands

The score is the weighted sum of the four factors, capped at 0–100:

| Band | Score | Operator action |
| --- | --- | --- |
| `Critical` | 0–39 | Act now — the product is failing core guarantees |
| `AtRisk` | 40–69 | Degraded — monitor closely |
| `Stable` | 70–89 | Healthy with room to improve |
| `Excellent` | 90–100 | Fully healthy |

Every `HealthReport` also carries the per-factor percentages, the weighted
point contributions, and a concrete `recommendation` string that always targets
the weakest factor (for example, pausing uploads and triggering repair when
replica durability collapses).

## Trend history

`HealthHistory` keeps a bounded rolling window (default 60 samples) of
`(timestamp, score)` pairs and exposes:

- `latest()` — the most recent score
- `average()` — window average
- `delta()` — signed change from the oldest to the newest sample

Out-of-order timestamps are ignored, so a clock jump can never corrupt the
trend.

## Example

```rust
use awep2p_core::product_health::{compute_report, HealthInputs, HealthWeights};

let inputs = HealthInputs {
    replica_health_pct: 60,
    peer_health_pct: 90,
    transfer_success_pct: 95,
    dht_responsiveness_pct: 80,
};
let report = compute_report(inputs, HealthWeights::default()).unwrap();
println!("score={} band={}", report.score, report.band.label());
println!("{}", report.recommendation);
```

## Status rule reminder

A good score is a *symptom-based signal*, not release evidence. Production
readiness still requires independent-node and clean-machine validation as
defined in [PRODUCT-READINESS-GATES.md](PRODUCT-READINESS-GATES.md) and
[REAL-NETWORK-TEST.md](REAL-NETWORK-TEST.md).
