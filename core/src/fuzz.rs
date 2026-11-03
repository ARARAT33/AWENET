//! Bounded deterministic fuzz harness for the simulator under chaos.
//!
//! Deterministic and replay-friendly: a run is fully described by its
//! configuration; the same seed replays the same traffic and fault
//! schedule and must produce an identical report. Conservation,
//! queue-bound, plan and node-count invariants are verified at the end of
//! every run.

use crate::chaos::{ChaosDriver, ChaosError, ChaosPlan, SplitMix64};
use crate::simulator::{Sim, SimError, MAX_INFLIGHT, MAX_LATENCY, MAX_NODES};
use thiserror::Error;

pub const MAX_FUZZ_TICKS: u64 = 10_000;
pub const MAX_FUZZ_SENDS_PER_TICK: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum FuzzError {
    #[error("seed must not be zero")]
    ZeroSeed,
    #[error("tick count must be between 1 and {MAX_FUZZ_TICKS}")]
    InvalidTicks,
    #[error("node count must be between 1 and {MAX_NODES}")]
    InvalidNodes,
    #[error("sends per tick must be between 1 and {MAX_FUZZ_SENDS_PER_TICK}")]
    InvalidSendsPerTick,
    #[error("intensity must stay between 0 and 100 percent")]
    InvalidIntensity,
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error(transparent)]
    Chaos(#[from] ChaosError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzConfig {
    pub seed: u64,
    pub ticks: u64,
    pub nodes: u64,
    pub sends_per_tick: u32,
    pub intensity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzReport {
    pub config: FuzzConfig,
    pub sent: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub blocked: u64,
    pub in_flight: usize,
    pub chaos_applied: u64,
    pub chaos_skipped: u64,
    pub violations: u32,
}

/// Runs one bounded fuzz campaign described entirely by the config.
pub fn run_fuzz(config: FuzzConfig) -> Result<FuzzReport, FuzzError> {
    if config.seed == 0 {
        return Err(FuzzError::ZeroSeed);
    }
    if config.ticks == 0 || config.ticks > MAX_FUZZ_TICKS {
        return Err(FuzzError::InvalidTicks);
    }
    if config.nodes == 0 || config.nodes > MAX_NODES as u64 {
        return Err(FuzzError::InvalidNodes);
    }
    if config.sends_per_tick == 0 || config.sends_per_tick > MAX_FUZZ_SENDS_PER_TICK {
        return Err(FuzzError::InvalidSendsPerTick);
    }
    if config.intensity > 100 {
        return Err(FuzzError::InvalidIntensity);
    }

    let mut sim = Sim::new(config.nodes as usize)?;
    let ids: Vec<u64> = (1..=config.nodes).collect();
    for &id in &ids {
        sim.add_node(id)?;
    }
    let plan = ChaosPlan::generate(
        config.seed.wrapping_add(1),
        config.ticks,
        config.intensity,
        &ids,
    )?;
    let mut driver = ChaosDriver::new(plan);
    let mut rng = SplitMix64::new(config.seed)?;
    let mut blocked = 0u64;

    for _ in 0..config.ticks {
        for _ in 0..config.sends_per_tick {
            let from = 1 + rng.below(config.nodes);
            let to = 1 + rng.below(config.nodes);
            let latency = 1 + rng.below(MAX_LATENCY);
            if sim.send(from, to, latency).is_err() {
                blocked += 1;
            }
        }
        sim.step();
        let now = sim.tick();
        driver.advance(&mut sim, now);
    }
    while sim.in_flight() > 0 {
        sim.step();
        let now = sim.tick();
        driver.advance(&mut sim, now);
    }

    let stats = sim.stats();
    let in_flight = sim.in_flight();
    let mut violations = 0u32;
    if stats.sent != stats.delivered + stats.dropped + in_flight as u64 {
        violations += 1;
    }
    if in_flight > MAX_INFLIGHT {
        violations += 1;
    }
    if driver.remaining() != 0 {
        violations += 1;
    }
    if sim.node_count() != config.nodes as usize {
        violations += 1;
    }

    Ok(FuzzReport {
        config,
        sent: stats.sent,
        delivered: stats.delivered,
        dropped: stats.dropped,
        blocked,
        in_flight,
        chaos_applied: driver.applied,
        chaos_skipped: driver.skipped,
        violations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(seed: u64) -> FuzzConfig {
        FuzzConfig { seed, ticks: 200, nodes: 5, sends_per_tick: 4, intensity: 40 }
    }

    #[test]
    fn rejects_invalid_configs() {
        let mut bad = config(1);
        bad.seed = 0;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::ZeroSeed);
        bad = config(1);
        bad.ticks = 0;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidTicks);
        bad = config(1);
        bad.ticks = MAX_FUZZ_TICKS + 1;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidTicks);
        bad = config(1);
        bad.nodes = 0;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidNodes);
        bad = config(1);
        bad.nodes = MAX_NODES as u64 + 1;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidNodes);
        bad = config(1);
        bad.sends_per_tick = 0;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidSendsPerTick);
        bad = config(1);
        bad.intensity = 101;
        assert_eq!(run_fuzz(bad).unwrap_err(), FuzzError::InvalidIntensity);
    }

    #[test]
    fn invariants_hold_across_seeds() {
        for seed in 1..=16u64 {
            let report = run_fuzz(config(seed)).unwrap();
            assert_eq!(report.violations, 0);
        }
    }

    #[test]
    fn same_seed_replays_identically() {
        let first = run_fuzz(config(42)).unwrap();
        let second = run_fuzz(config(42)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn zero_intensity_is_lossless() {
        let mut calm = config(7);
        calm.intensity = 0;
        let report = run_fuzz(calm).unwrap();
        assert_eq!(report.blocked, 0);
        assert_eq!(report.dropped, 0);
        assert_eq!(report.delivered, report.sent);
        assert_eq!(report.violations, 0);
    }
}
