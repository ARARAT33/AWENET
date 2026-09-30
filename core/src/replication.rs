//! Storage placement and replica-health primitives.
//! A file can be represented as 1000 erasure shards, with each shard assigned
//! to three distinct nodes. Placement is deterministic for a given file ID,
//! shard index and node set, but it never invents node IDs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const REQUIRED_SHARDS: usize = 1000;
pub const REQUIRED_REPLICAS: usize = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicaPlacement {
    pub shard_index: u16,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlacementPlan {
    pub file_id: [u8; 32],
    pub shards: usize,
    pub replicas_per_shard: usize,
    pub placements: Vec<ReplicaPlacement>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicaHealth {
    pub shard_index: u16,
    pub available_nodes: Vec<String>,
    pub missing_replicas: usize,
    pub healthy: bool,
}

pub fn select_replicas(file_id: &[u8; 32], shard_index: usize, nodes: &[String]) -> Vec<String> {
    let mut candidates: Vec<String> = nodes.iter().filter(|n| !n.is_empty()).cloned().collect();
    candidates.sort();
    candidates.dedup();
    if candidates.len() <= REQUIRED_REPLICAS {
        return candidates;
    }
    candidates.sort_by_key(|node| {
        let mut seed = Vec::with_capacity(32 + 8 + node.len());
        seed.extend_from_slice(file_id);
        seed.extend_from_slice(&(shard_index as u64).to_be_bytes());
        seed.extend_from_slice(node.as_bytes());
        *blake3::hash(&seed).as_bytes()
    });
    candidates.truncate(REQUIRED_REPLICAS);
    candidates
}

pub fn build_plan(file_id: [u8; 32], nodes: &[String]) -> Result<PlacementPlan, String> {
    let mut unique = BTreeSet::new();
    for n in nodes {
        if !n.is_empty() {
            unique.insert(n.clone());
        }
    }
    if unique.len() < REQUIRED_REPLICAS {
        return Err("at least three distinct storage nodes are required".into());
    }
    let candidates: Vec<String> = unique.into_iter().collect();
    let placements = (0..REQUIRED_SHARDS)
        .map(|i| ReplicaPlacement {
            shard_index: i as u16,
            nodes: select_replicas(&file_id, i, &candidates),
        })
        .collect();
    Ok(PlacementPlan {
        file_id,
        shards: REQUIRED_SHARDS,
        replicas_per_shard: REQUIRED_REPLICAS,
        placements,
    })
}

pub fn assess(plan: &PlacementPlan, online_nodes: &BTreeSet<String>) -> Vec<ReplicaHealth> {
    plan.placements
        .iter()
        .map(|p| {
            let available_nodes: Vec<_> = p.nodes.iter().filter(|n| online_nodes.contains(*n)).cloned().collect();
            let missing_replicas = REQUIRED_REPLICAS.saturating_sub(available_nodes.len());
            ReplicaHealth {
                shard_index: p.shard_index,
                available_nodes,
                missing_replicas,
                healthy: missing_replicas == 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_has_exactly_1000_shards_and_three_replicas() {
        let nodes: Vec<_> = (0..12).map(|i| format!("node-{i}")).collect();
        let plan = build_plan([7; 32], &nodes).unwrap();
        assert_eq!(plan.shards, 1000);
        assert_eq!(plan.placements.len(), 1000);
        assert!(plan.placements.iter().all(|p| p.nodes.len() == 3));
    }

    #[test]
    fn placement_is_deterministic() {
        let nodes: Vec<_> = (0..8).map(|i| format!("node-{i}")).collect();
        let a = build_plan([8; 32], &nodes).unwrap();
        let b = build_plan([8; 32], &nodes).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn health_detects_missing_replicas() {
        let nodes: Vec<_> = (0..4).map(|i| format!("node-{i}")).collect();
        let plan = build_plan([9; 32], &nodes).unwrap();
        let online = BTreeSet::from([plan.placements[0].nodes[0].clone()]);
        let h = assess(&plan, &online);
        assert_eq!(h[0].available_nodes.len(), 1);
        assert_eq!(h[0].missing_replicas, 2);
        assert!(!h[0].healthy);
    }
}
