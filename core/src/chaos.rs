//! Deterministic chaos engineering on top of the network simulator.
//!
//! Deterministic and replay-friendly: fault plans are generated from an
//! explicit seed through a splitmix64 stream, so the same seed always
//! yields the same fault schedule. Every node failure generated within the
//! horizon is healed by the end of the plan, which keeps liveness verdicts
//! meaningful.

use crate::simulator::{Sim, SimError};
use thiserror::Error;

pub const MAX_HORIZON: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ChaosError {
    #[error("seed must not be zero")]
    ZeroSeed,
    #[error("horizon must be between 1 and {MAX_HORIZON} ticks")]
    InvalidHorizon,
    #[error("intensity must stay between 0 and 100 percent")]
    InvalidIntensity,
    #[error("chaos needs at least one node")]
    EmptyNodeSet,
    #[error(transparent)]
    Sim(#[from] SimError),
}

/// A splitmix64 stream; the same seed always yields the same sequence.
#[derive(Clone, Copy, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a new stream from an explicit non-zero seed.
    pub fn new(seed: u64) -> Result<Self, ChaosError> {
        if seed == 0 {
            return Err(ChaosError::ZeroSeed);
        }
        Ok(Self { state: seed })
    }

    /// Advances the stream and returns the next 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value below bound (zero when bound is zero).
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }
}

/// A single fault applied to a simulation at a known tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaosEvent {
    DropInflight { node: u64 },
    TakeOffline { node: u64 },
    BringOnline { node: u64 },
}

/// A fault schedule: (tick, event) pairs sorted by tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChaosPlan {
    pub events: Vec<(u64, ChaosEvent)>,
}

impl ChaosPlan {
    /// Generates a fault schedule: drops and bounded node failures whose
    /// heal is always scheduled within the horizon.
    pub fn generate(
        seed: u64,
        horizon: u64,
        intensity: u32,
        node_ids: &[u64],
    ) -> Result<Self, ChaosError> {
        if seed == 0 {
            return Err(ChaosError::ZeroSeed);
        }
        if horizon == 0 || horizon > MAX_HORIZON {
            return Err(ChaosError::InvalidHorizon);
        }
        if intensity > 100 {
            return Err(ChaosError::InvalidIntensity);
        }
        if node_ids.is_empty() {
            return Err(ChaosError::EmptyNodeSet);
        }
        let mut rng = SplitMix64::new(seed)?;
        let mut events = Vec::new();
        for tick in 1..=horizon {
            if rng.below(100) >= u64::from(intensity) {
                continue;
            }
            let node = node_ids[rng.below(node_ids.len() as u64) as usize];
            match rng.below(4) {
                0 => events.push((tick, ChaosEvent::DropInflight { node })),
                _ => {
                    let downtime = 1 + rng.below(3);
                    events.push((tick, ChaosEvent::TakeOffline { node }));
                    let heal_tick = (tick + downtime).min(horizon);
                    events.push((heal_tick, ChaosEvent::BringOnline { node }));
                }
            }
        }
        events.sort_by_key(|(tick, _)| *tick);
        Ok(Self { events })
    }
}

/// Whether a fault could be applied to the current simulation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Skipped,
}

/// Applies one fault to a simulation.
pub fn apply_event(sim: &mut Sim, event: ChaosEvent) -> ApplyOutcome {
    match event {
        ChaosEvent::DropInflight { node } => match sim.drop_inflight_to(node) {
            Ok(_) => ApplyOutcome::Applied,
            Err(_) => ApplyOutcome::Skipped,
        },
        ChaosEvent::TakeOffline { node } => match sim.take_offline(node) {
            Ok(_) => ApplyOutcome::Applied,
            Err(_) => ApplyOutcome::Skipped,
        },
        ChaosEvent::BringOnline { node } => match sim.bring_online(node) {
            Ok(()) => ApplyOutcome::Applied,
            Err(_) => ApplyOutcome::Skipped,
        },
    }
}

/// Applies a fault plan to a simulation in tick order.
#[derive(Debug)]
pub struct ChaosDriver {
    plan: ChaosPlan,
    cursor: usize,
    pub applied: u64,
    pub skipped: u64,
}

impl ChaosDriver {
    pub fn new(plan: ChaosPlan) -> Self {
        Self {
            plan,
            cursor: 0,
            applied: 0,
            skipped: 0,
        }
    }

    /// Applies every plan event scheduled at or before now.
    pub fn advance(&mut self, sim: &mut Sim, now: u64) {
        while self.cursor < self.plan.events.len() && self.plan.events[self.cursor].0 <= now {
            let (_, event) = self.plan.events[self.cursor];
            match apply_event(sim, event) {
                ApplyOutcome::Applied => self.applied += 1,
                ApplyOutcome::Skipped => self.skipped += 1,
            }
            self.cursor += 1;
        }
    }

    /// Plan events that have not been applied yet.
    pub fn remaining(&self) -> usize {
        self.plan.events.len() - self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_generation() {
        assert_eq!(
            ChaosPlan::generate(0, 10, 10, &[1, 2]).unwrap_err(),
            ChaosError::ZeroSeed
        );
        assert_eq!(
            ChaosPlan::generate(7, 0, 10, &[1, 2]).unwrap_err(),
            ChaosError::InvalidHorizon
        );
        assert_eq!(
            ChaosPlan::generate(7, MAX_HORIZON + 1, 10, &[1, 2]).unwrap_err(),
            ChaosError::InvalidHorizon
        );
        assert_eq!(
            ChaosPlan::generate(7, 10, 101, &[1, 2]).unwrap_err(),
            ChaosError::InvalidIntensity
        );
        assert_eq!(
            ChaosPlan::generate(7, 10, 10, &[]).unwrap_err(),
            ChaosError::EmptyNodeSet
        );
    }

    #[test]
    fn prng_is_deterministic() {
        let mut a = SplitMix64::new(1).unwrap();
        let mut b = SplitMix64::new(1).unwrap();
        let mut c = SplitMix64::new(2).unwrap();
        let seq_a: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let seq_c: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(seq_a, seq_b);
        assert_ne!(seq_a, seq_c);
        assert_eq!(SplitMix64::new(0).unwrap_err(), ChaosError::ZeroSeed);
    }

    #[test]
    fn plan_generation_is_deterministic() {
        let ids = [1u64, 2, 3];
        let a = ChaosPlan::generate(11, 60, 30, &ids).unwrap();
        let b = ChaosPlan::generate(11, 60, 30, &ids).unwrap();
        assert_eq!(a, b);
        assert!(!a.events.is_empty());
    }

    #[test]
    fn every_failure_is_healed_by_horizon() {
        let ids = [1u64, 2, 3, 4];
        let plan = ChaosPlan::generate(23, 50, 100, &ids).unwrap();
        let mut sim = Sim::new(4).unwrap();
        for &id in &ids {
            sim.add_node(id).unwrap();
        }
        let mut driver = ChaosDriver::new(plan);
        driver.advance(&mut sim, 50);
        assert_eq!(driver.remaining(), 0);
        assert!(driver.applied >= 50);
        assert_eq!(driver.skipped, 0);
        assert_eq!(sim.online_count(), 4);
    }

    #[test]
    fn driver_applies_events_in_tick_order() {
        let mut sim = Sim::new(2).unwrap();
        sim.add_node(1).unwrap();
        sim.add_node(2).unwrap();
        sim.send(1, 2, 4).unwrap();
        let plan = ChaosPlan {
            events: vec![
                (1, ChaosEvent::DropInflight { node: 2 }),
                (2, ChaosEvent::TakeOffline { node: 2 }),
                (5, ChaosEvent::BringOnline { node: 2 }),
            ],
        };
        let mut driver = ChaosDriver::new(plan);
        driver.advance(&mut sim, 1);
        assert_eq!(driver.applied, 1);
        assert_eq!(sim.in_flight(), 0);
        assert_eq!(sim.stats().dropped, 1);
        driver.advance(&mut sim, 2);
        assert_eq!(sim.is_online(2), Ok(false));
        assert_eq!(sim.send(1, 2, 1).unwrap_err(), SimError::NodeOffline(2));
        driver.advance(&mut sim, 5);
        assert_eq!(sim.is_online(2), Ok(true));
        assert_eq!(driver.remaining(), 0);
        assert_eq!(driver.applied, 3);
        assert_eq!(driver.skipped, 0);
    }
}
