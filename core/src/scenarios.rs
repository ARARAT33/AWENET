//! Ready-made simulation scenarios over the simulator and chaos layer.
//!
//! Deterministic and replay-friendly: each scenario is a fixed script of
//! sends, tick advances and fault events. A run returns a report with
//! traffic counters and a liveness verdict derived from structural
//! invariants: every sent envelope is delivered, in flight or dropped.

use crate::chaos::{ChaosDriver, ChaosError, ChaosPlan, SplitMix64};
use crate::simulator::{Sim, SimError};
use thiserror::Error;

pub const MAX_SCENARIO_NODES: u64 = 32;
pub const MAX_SCENARIO_TICKS: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ScenarioError {
    #[error("scenarios need between 1 and {MAX_SCENARIO_NODES} nodes")]
    InvalidNodeCount,
    #[error("horizon must be between 1 and {MAX_SCENARIO_TICKS} ticks")]
    InvalidHorizon,
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error(transparent)]
    Chaos(#[from] ChaosError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    SteadyDelivery,
    PartitionHealing,
    NodeChurn,
    MessageStorm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioReport {
    pub name: &'static str,
    pub ticks: u64,
    pub sent: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub blocked: u64,
    pub chaos_applied: u64,
    pub chaos_skipped: u64,
    pub nodes_total: usize,
    pub nodes_online: usize,
    pub liveness: bool,
}

/// Runs a named scenario with the given fleet size and horizon.
pub fn run(scenario: Scenario, nodes: u64, horizon: u64) -> Result<ScenarioReport, ScenarioError> {
    if nodes == 0 || nodes > MAX_SCENARIO_NODES {
        return Err(ScenarioError::InvalidNodeCount);
    }
    if horizon == 0 || horizon > MAX_SCENARIO_TICKS {
        return Err(ScenarioError::InvalidHorizon);
    }
    match scenario {
        Scenario::SteadyDelivery => steady_delivery(nodes, horizon),
        Scenario::PartitionHealing => partition_healing(nodes, horizon),
        Scenario::NodeChurn => node_churn(nodes, horizon),
        Scenario::MessageStorm => message_storm(nodes, horizon),
    }
}

fn build_sim(nodes: u64) -> Result<Sim, ScenarioError> {
    let mut sim = Sim::new(nodes as usize)?;
    for id in 1..=nodes {
        sim.add_node(id)?;
    }
    Ok(sim)
}

fn conserved(sim: &Sim) -> bool {
    let stats = sim.stats();
    stats.sent == stats.delivered + stats.dropped + sim.in_flight() as u64
}

#[allow(clippy::too_many_arguments)]
fn finish(
    name: &'static str,
    horizon: u64,
    sim: &Sim,
    blocked: u64,
    applied: u64,
    skipped: u64,
    liveness: bool,
) -> ScenarioReport {
    let stats = sim.stats();
    ScenarioReport {
        name,
        ticks: horizon,
        sent: stats.sent,
        delivered: stats.delivered,
        dropped: stats.dropped,
        blocked,
        chaos_applied: applied,
        chaos_skipped: skipped,
        nodes_total: sim.node_count(),
        nodes_online: sim.online_count(),
        liveness,
    }
}

fn steady_delivery(nodes: u64, horizon: u64) -> Result<ScenarioReport, ScenarioError> {
    let mut sim = build_sim(nodes)?;
    let mut blocked = 0u64;
    for tick in 0..horizon {
        let from = 1 + (tick % nodes);
        let to = 1 + ((tick + 1) % nodes);
        if sim.send(from, to, 1).is_err() {
            blocked += 1;
        }
        sim.step();
    }
    sim.drain();
    let liveness = conserved(&sim) && sim.stats().delivered == sim.stats().sent;
    Ok(finish(
        "steady-delivery",
        horizon,
        &sim,
        blocked,
        0,
        0,
        liveness,
    ))
}

fn partition_healing(nodes: u64, horizon: u64) -> Result<ScenarioReport, ScenarioError> {
    let mut sim = build_sim(nodes)?;
    let ids: Vec<u64> = (1..=nodes).collect();
    let plan = ChaosPlan::generate(11, horizon, 30, &ids)?;
    let mut driver = ChaosDriver::new(plan);
    let mut blocked = 0u64;
    for tick in 0..horizon {
        let from = 1 + (tick % nodes);
        let to = 1 + ((tick + 1) % nodes);
        if sim.send(from, to, 1).is_err() {
            blocked += 1;
        }
        sim.step();
        let now = sim.tick();
        driver.advance(&mut sim, now);
    }
    sim.drain();
    let liveness = conserved(&sim) && sim.online_count() == sim.node_count();
    Ok(finish(
        "partition-healing",
        horizon,
        &sim,
        blocked,
        driver.applied,
        driver.skipped,
        liveness,
    ))
}

fn node_churn(nodes: u64, horizon: u64) -> Result<ScenarioReport, ScenarioError> {
    let mut sim = build_sim(nodes)?;
    let mut blocked = 0u64;
    for tick in 0..horizon {
        let from = 1 + (tick % nodes);
        let to = 1 + ((tick + 1) % nodes);
        if sim.send(from, to, 1).is_err() {
            blocked += 1;
        }
        sim.step();
        if (tick + 1) % 8 == 0 {
            let victim = 1 + ((tick / 8) % nodes);
            sim.remove_node(victim)?;
            sim.add_node(victim)?;
        }
    }
    sim.drain();
    let liveness = conserved(&sim) && sim.stats().delivered == sim.stats().sent;
    Ok(finish("node-churn", horizon, &sim, blocked, 0, 0, liveness))
}

fn message_storm(nodes: u64, horizon: u64) -> Result<ScenarioReport, ScenarioError> {
    let mut sim = build_sim(nodes)?;
    let mut rng = SplitMix64::new(99)?;
    let mut blocked = 0u64;
    for _ in 0..horizon {
        loop {
            let from = 1 + rng.below(nodes);
            let to = 1 + rng.below(nodes);
            if sim.send(from, to, 4).is_err() {
                blocked += 1;
                break;
            }
        }
        sim.step();
    }
    sim.drain();
    let liveness = conserved(&sim) && sim.stats().delivered == sim.stats().sent;
    Ok(finish(
        "message-storm",
        horizon,
        &sim,
        blocked,
        0,
        0,
        liveness,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_arguments() {
        assert_eq!(
            run(Scenario::SteadyDelivery, 0, 10).unwrap_err(),
            ScenarioError::InvalidNodeCount
        );
        assert_eq!(
            run(Scenario::SteadyDelivery, MAX_SCENARIO_NODES + 1, 10).unwrap_err(),
            ScenarioError::InvalidNodeCount
        );
        assert_eq!(
            run(Scenario::NodeChurn, 4, 0).unwrap_err(),
            ScenarioError::InvalidHorizon
        );
        assert_eq!(
            run(Scenario::NodeChurn, 4, MAX_SCENARIO_TICKS + 1).unwrap_err(),
            ScenarioError::InvalidHorizon
        );
    }

    #[test]
    fn steady_delivery_is_lossless() {
        let report = run(Scenario::SteadyDelivery, 4, 40).unwrap();
        assert_eq!(report.name, "steady-delivery");
        assert_eq!(report.sent, 40);
        assert_eq!(report.delivered, 40);
        assert_eq!(report.dropped, 0);
        assert_eq!(report.blocked, 0);
        assert_eq!(report.nodes_online, 4);
        assert!(report.liveness);
    }

    #[test]
    fn partition_healing_recovers_all_nodes() {
        let report = run(Scenario::PartitionHealing, 5, 60).unwrap();
        assert_eq!(report.nodes_total, 5);
        assert_eq!(report.nodes_online, 5);
        assert_eq!(report.chaos_skipped, 0);
        assert!(report.liveness);
    }

    #[test]
    fn node_churn_keeps_the_fleet_alive() {
        let report = run(Scenario::NodeChurn, 4, 40).unwrap();
        assert_eq!(report.nodes_total, 4);
        assert_eq!(report.nodes_online, 4);
        assert_eq!(report.delivered, report.sent);
        assert_eq!(report.blocked, 0);
        assert!(report.liveness);
    }

    #[test]
    fn message_storm_bursts_then_drains() {
        let report = run(Scenario::MessageStorm, 3, 20).unwrap();
        assert!(report.sent >= 4_096);
        assert!(report.blocked >= 20);
        assert_eq!(report.dropped, 0);
        assert_eq!(report.delivered, report.sent);
        assert!(report.liveness);
    }
}
