//! Deterministic, iterative peer routing primitives for AWEP2P.
//! This module deliberately separates logical routing distance from geographic
//! distance. Production transports can layer on top of these primitives.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRoute {
    pub node_id: String,
    pub address: String,
    pub distance: u128,
    pub latency_ms: Option<u32>,
    pub healthy: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LookupRequest {
    pub request_id: [u8; 16],
    pub target: [u8; 32],
    pub max_hops: u16,
    pub fanout: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LookupResult {
    pub target: [u8; 32],
    pub peers: Vec<PeerRoute>,
    pub complete: bool,
    pub hops: u16,
}

pub fn xor_distance(a: &[u8; 32], b: &[u8; 32]) -> u128 {
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = a[i] ^ b[i];
    }
    u128::from_be_bytes(out)
}

pub fn rank_peers(target: &[u8; 32], peers: &[PeerRoute]) -> Vec<PeerRoute> {
    let mut ranked: Vec<_> = peers
        .iter()
        .filter(|p| p.healthy && !p.node_id.is_empty())
        .cloned()
        .collect();
    ranked.sort_by_key(|p| (p.distance, p.latency_ms.unwrap_or(u32::MAX), p.node_id.clone()));
    ranked
        .into_iter()
        .map(|mut p| {
            // Recompute distance from the node descriptor when it is a 64-char hex ID.
            if let Ok(bytes) = hex::decode(&p.node_id) {
                if bytes.len() == 32 {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(&bytes);
                    p.distance = xor_distance(target, &id);
                }
            }
            p
        })
        .collect()
}

pub fn next_hop(
    target: &[u8; 32],
    peers: &[PeerRoute],
    visited: &BTreeSet<String>,
) -> Option<PeerRoute> {
    rank_peers(target, peers)
        .into_iter()
        .find(|p| !visited.contains(&p.node_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: u8, latency: u32) -> PeerRoute {
        PeerRoute {
            node_id: hex::encode([id; 32]),
            address: format!("127.0.0.1:{}", 4000 + id as u16),
            distance: 0,
            latency_ms: Some(latency),
            healthy: true,
        }
    }

    #[test]
    fn xor_distance_is_deterministic() {
        assert_eq!(xor_distance(&[0; 32], &[0; 32]), 0);
        assert_eq!(xor_distance(&[1; 32], &[0; 32]), u128::MAX);
    }

    #[test]
    fn next_hop_avoids_visited_peers() {
        let target = [2u8; 32];
        let peers = vec![peer(2, 20), peer(3, 10)];
        let mut visited = BTreeSet::new();
        visited.insert(hex::encode([2u8; 32]));
        assert_eq!(next_hop(&target, &peers, &visited).unwrap().node_id, hex::encode([3u8; 32]));
    }

    #[test]
    fn unhealthy_peers_are_not_candidates() {
        let mut p = peer(9, 1);
        p.healthy = false;
        assert!(next_hop(&[9u8; 32], &[p], &BTreeSet::new()).is_none());
    }
}
