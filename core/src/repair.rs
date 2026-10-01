use crate::replication::{PlacementPlan, ReplicaHealth, REQUIRED_REPLICAS};
use std::collections::BTreeSet;

pub fn plan_repairs(
    plan: &PlacementPlan,
    health: &[ReplicaHealth],
    online_nodes: &BTreeSet<String>,
) -> Vec<(u16, Vec<String>)> {
    let mut out = Vec::new();
    for h in health.iter().filter(|h| h.missing_replicas > 0) {
        if let Some(p) = plan
            .placements
            .iter()
            .find(|p| p.shard_index == h.shard_index)
        {
            let used: BTreeSet<_> = p.nodes.iter().cloned().collect();
            let mut candidates: Vec<_> = online_nodes
                .iter()
                .filter(|n| !used.contains(*n))
                .cloned()
                .collect();
            candidates.sort();
            let take = REQUIRED_REPLICAS.saturating_sub(h.available_nodes.len());
            candidates.truncate(take);
            if candidates.len() == take {
                out.push((h.shard_index, candidates));
            }
        }
    }
    out
}

pub struct ExecutionRepairResult {
    pub file_id: [u8; 32],
    pub repaired_shards: usize,
    pub failed_shards: usize,
}

pub fn execute_repairs(
    file_id: [u8; 32],
    plan: &PlacementPlan,
    health: &[ReplicaHealth],
    online_nodes: &BTreeSet<String>,
) -> ExecutionRepairResult {
    let tasks = plan_repairs(plan, health, online_nodes);
    let mut repaired = 0;
    let mut failed = 0;
    for (_shard_index, candidates) in tasks {
        if candidates.is_empty() {
            failed += 1;
        } else {
            repaired += 1;
        }
    }
    ExecutionRepairResult {
        file_id,
        repaired_shards: repaired,
        failed_shards: failed,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::build_plan;
    #[test]
    fn repairs_missing_replicas() {
        let ns: Vec<_> = (0..5).map(|i| format!("n{i}")).collect();
        let p = build_plan([1; 32], &ns).unwrap();
        let online = BTreeSet::from_iter(
            p.placements[0].nodes[..2]
                .iter()
                .cloned()
                .chain(std::iter::once("n4".into())),
        );
        let h = crate::replication::assess(&p, &online);
        let repairs = plan_repairs(&p, &h, &online);
        assert!(!repairs.is_empty());
        let exec = execute_repairs([1; 32], &p, &h, &online);
        assert_eq!(exec.file_id, [1; 32]);
    }
}
