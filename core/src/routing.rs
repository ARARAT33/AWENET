//! Deterministic, iterative peer routing primitives for AWEP2P.
//! This module deliberately separates logical routing distance from geographic
//! distance. Production transports can layer on top of these primitives.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRoute {
    pub node_id: String,
    pub address: String,
    pub distance: [u8; 32],
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

pub fn xor_distance(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[i];
    }
    out
}

fn node_id_bytes(node_id: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(node_id).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Some(id)
}

pub fn rank_peers(target: &[u8; 32], peers: &[PeerRoute]) -> Vec<PeerRoute> {
    let mut ranked: Vec<_> = peers
        .iter()
        .filter(|p| p.healthy && !p.node_id.is_empty())
        .cloned()
        .map(|mut p| {
            if let Some(id) = node_id_bytes(&p.node_id) {
                p.distance = xor_distance(target, &id);
            }
            p
        })
        .collect();
    ranked.sort_by_key(|p| {
        (
            p.distance,
            p.latency_ms.unwrap_or(u32::MAX),
            p.node_id.clone(),
        )
    });
    ranked
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteHop {
    pub hop_index: u16,
    pub node_id: String,
    pub address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiHopRoutePlanner {
    pub max_hops: u16,
}

impl MultiHopRoutePlanner {
    pub fn new(max_hops: u16) -> Self {
        Self { max_hops }
    }

    pub fn plan_route(
        &self,
        target: &[u8; 32],
        available_peers: &[PeerRoute],
    ) -> Result<Vec<RouteHop>, String> {
        if available_peers.is_empty() {
            return Err("no candidate peers for multi-hop route".into());
        }
        let ranked = rank_peers(target, available_peers);
        if ranked.is_empty() {
            return Err("no healthy candidate peers for multi-hop route".into());
        }
        let mut visited = BTreeSet::new();
        let mut route = Vec::new();

        for hop_index in 0..self.max_hops {
            if let Some(candidate) = next_hop(target, &ranked, &visited) {
                visited.insert(candidate.node_id.clone());
                route.push(RouteHop {
                    hop_index,
                    node_id: candidate.node_id.clone(),
                    address: candidate.address,
                });
                if candidate.distance == [0u8; 32] {
                    break;
                }
            } else {
                break;
            }
        }

        if route.is_empty() {
            Err("failed to establish valid multi-hop path".into())
        } else {
            Ok(route)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: u8, latency: u32) -> PeerRoute {
        PeerRoute {
            node_id: hex::encode([id; 32]),
            address: format!("127.0.0.1:{}", 4000 + id as u16),
            distance: [0; 32],
            latency_ms: Some(latency),
            healthy: true,
        }
    }

    #[test]
    fn xor_distance_is_deterministic() {
        assert_eq!(xor_distance(&[0; 32], &[0; 32]), [0; 32]);
        assert_eq!(xor_distance(&[1; 32], &[0; 32]), [1; 32]);
    }

    #[test]
    fn next_hop_avoids_visited_peers() {
        let target = [2u8; 32];
        let peers = vec![peer(2, 20), peer(3, 10)];
        let mut visited = BTreeSet::new();
        visited.insert(hex::encode([2u8; 32]));
        assert_eq!(
            next_hop(&target, &peers, &visited).unwrap().node_id,
            hex::encode([3u8; 32])
        );
    }

    #[test]
    fn unhealthy_peers_are_not_candidates() {
        let mut p = peer(9, 1);
        p.healthy = false;
        assert!(next_hop(&[9u8; 32], &[p], &BTreeSet::new()).is_none());
    }

    #[test]
    fn multi_hop_route_planner_calculates_hops() {
        let planner = MultiHopRoutePlanner::new(3);
        let target = [5u8; 32];
        let peers = vec![peer(1, 10), peer(2, 20), peer(5, 5)];
        let route = planner.plan_route(&target, &peers).unwrap();
        assert!(!route.is_empty());
        assert!(route.len() <= 3);
    }
}

#[cfg(test)]
mod routing_distance_regression {
    use super::*;
    #[test]
    fn uses_full_256_bit_distance() {
        let target = [0u8; 32];
        let mut low = [0u8; 32];
        low[31] = 1;
        let mut high = [0u8; 32];
        high[0] = 1;
        let peers = vec![
            PeerRoute {
                node_id: hex::encode(low),
                address: "127.0.0.1:1".into(),
                distance: [0; 32],
                latency_ms: Some(1),
                healthy: true,
            },
            PeerRoute {
                node_id: hex::encode(high),
                address: "127.0.0.1:2".into(),
                distance: [0; 32],
                latency_ms: Some(1),
                healthy: true,
            },
        ];
        assert_eq!(rank_peers(&target, &peers)[0].node_id, hex::encode(low));
    }
}
