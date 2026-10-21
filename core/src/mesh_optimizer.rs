//! # AWENET Mesh Optimizer Module
//! Provides adaptive latency tracking, peer health telemetry, and dynamic route scoring
//! for decentralized P2P communication nodes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Represents health metrics and telemetry for a connected peer node in the mesh.
#[derive(Debug, Clone)]
pub struct PeerTelemetry {
    pub peer_id: String,
    pub latency: Duration,
    pub packet_loss_rate: f32,
    pub successful_pings: u64,
    pub failed_pings: u64,
    pub last_seen: Instant,
}

impl PeerTelemetry {
    pub fn new(peer_id: impl Into<String>) -> Self {
        Self {
            peer_id: peer_id.into(),
            latency: Duration::from_millis(50),
            packet_loss_rate: 0.0,
            successful_pings: 0,
            failed_pings: 0,
            last_seen: Instant::now(),
        }
    }

    /// Calculates a dynamic routing score based on latency and packet loss.
    /// Lower score indicates a healthier and faster route.
    pub fn compute_route_score(&self) -> f32 {
        let latency_ms = self.latency.as_secs_f32() * 1000.0;
        let loss_penalty = self.packet_loss_rate * 500.0;
        latency_ms + loss_penalty
    }
}

/// Manages mesh routing metrics across all connected peers.
#[derive(Debug, Default)]
pub struct MeshOptimizer {
    peers: HashMap<String, PeerTelemetry>,
}

impl MeshOptimizer {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Registers or updates telemetry data for a peer.
    pub fn update_peer(&mut self, peer_id: &str, latency: Duration, success: bool) {
        let entry = self.peers.entry(peer_id.to_string()).or_insert_with(|| PeerTelemetry::new(peer_id));
        entry.last_seen = Instant::now();
        entry.latency = latency;
        if success {
            entry.successful_pings += 1;
            // Decay packet loss slightly on success
            entry.packet_loss_rate = (entry.packet_loss_rate * 0.9).max(0.0);
        } else {
            entry.failed_pings += 1;
            // Increase packet loss on failure
            entry.packet_loss_rate = (entry.packet_loss_rate + 0.1).min(1.0);
        }
    }

    /// Selects the optimal peer route based on the lowest computed routing score.
    pub fn get_optimal_peer(&self) -> Option<String> {
        self.peers
            .iter()
            .min_by(|a, b| {
                let score_a = a.1.compute_route_score();
                let score_b = b.1.compute_route_score();
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| id.clone())
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_optimizer_routing() {
        let mut optimizer = MeshOptimizer::new();
        optimizer.update_peer("node_alpha", Duration::from_millis(30), true);
        optimizer.update_peer("node_beta", Duration::from_millis(120), true);

        let optimal = optimizer.get_optimal_peer();
        assert_eq!(optimal, Some("node_alpha".to_string()));
    }
}
