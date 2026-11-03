//! Deterministic discrete-tick network simulator.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time advances only through explicit step and run_until calls.
//! Envelopes are scheduled for a future tick and delivered in tick order,
//! so identical operation sequences always produce identical results.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_NODES: usize = 128;
pub const MAX_INFLIGHT: usize = 4_096;
pub const MAX_LATENCY: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SimError {
    #[error("simulator capacity must be between 1 and {MAX_NODES} nodes")]
    InvalidCapacity,
    #[error("simulator is full (max {MAX_NODES} nodes)")]
    SimFull,
    #[error("node {0} already exists")]
    NodeExists(u64),
    #[error("unknown node {0}")]
    UnknownNode(u64),
    #[error("node {0} is offline")]
    NodeOffline(u64),
    #[error("latency must stay between 1 and {MAX_LATENCY} ticks")]
    InvalidLatency,
    #[error("message queue is full (max {MAX_INFLIGHT} in flight)")]
    QueueFull,
    #[error("time cannot move backwards from tick {0}")]
    TimeWarp(u64),
}

/// A node with an explicit online flag driven by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeState {
    pub id: u64,
    pub online: bool,
}

/// A message scheduled for delivery at a future tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub id: u64,
    pub from: u64,
    pub to: u64,
    pub sent_tick: u64,
    pub deliver_tick: u64,
}

/// Traffic counters; sent is the source of conservation checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimStats {
    pub sent: u64,
    pub delivered: u64,
    pub dropped: u64,
}

#[derive(Debug)]
pub struct Sim {
    max_nodes: usize,
    tick: u64,
    next_envelope: u64,
    nodes: BTreeMap<u64, NodeState>,
    scheduled: BTreeMap<u64, Vec<Envelope>>,
    stats: SimStats,
}

impl Sim {
    pub fn new(max_nodes: usize) -> Result<Self, SimError> {
        if max_nodes == 0 || max_nodes > MAX_NODES {
            return Err(SimError::InvalidCapacity);
        }
        Ok(Self {
            max_nodes,
            tick: 0,
            next_envelope: 1,
            nodes: BTreeMap::new(),
            scheduled: BTreeMap::new(),
            stats: SimStats::default(),
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn has_node(&self, id: u64) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn is_online(&self, id: u64) -> Result<bool, SimError> {
        match self.nodes.get(&id) {
            None => Err(SimError::UnknownNode(id)),
            Some(node) => Ok(node.online),
        }
    }

    pub fn online_count(&self) -> usize {
        self.nodes.values().filter(|node| node.online).count()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn stats(&self) -> SimStats {
        self.stats
    }

    /// Envelopes scheduled but not delivered yet.
    pub fn in_flight(&self) -> usize {
        self.scheduled.values().map(Vec::len).sum()
    }

    pub fn add_node(&mut self, id: u64) -> Result<(), SimError> {
        if self.nodes.contains_key(&id) {
            return Err(SimError::NodeExists(id));
        }
        if self.nodes.len() >= self.max_nodes {
            return Err(SimError::SimFull);
        }
        self.nodes.insert(id, NodeState { id, online: true });
        Ok(())
    }

    /// Removes a node and returns the in-flight envelopes dropped with it.
    pub fn remove_node(&mut self, id: u64) -> Result<u64, SimError> {
        if self.nodes.remove(&id).is_none() {
            return Err(SimError::UnknownNode(id));
        }
        Ok(self.drop_inflight_to_inner(id))
    }

    /// Takes a node offline and drops the envelopes addressed to it.
    pub fn take_offline(&mut self, id: u64) -> Result<u64, SimError> {
        match self.nodes.get_mut(&id) {
            None => Err(SimError::UnknownNode(id)),
            Some(node) => {
                node.online = false;
                Ok(self.drop_inflight_to_inner(id))
            }
        }
    }

    pub fn bring_online(&mut self, id: u64) -> Result<(), SimError> {
        match self.nodes.get_mut(&id) {
            None => Err(SimError::UnknownNode(id)),
            Some(node) => {
                node.online = true;
                Ok(())
            }
        }
    }

    /// Drops the in-flight envelopes addressed to a known node.
    pub fn drop_inflight_to(&mut self, id: u64) -> Result<u64, SimError> {
        if !self.nodes.contains_key(&id) {
            return Err(SimError::UnknownNode(id));
        }
        Ok(self.drop_inflight_to_inner(id))
    }

    fn drop_inflight_to_inner(&mut self, id: u64) -> u64 {
        let mut dropped = 0u64;
        for batch in self.scheduled.values_mut() {
            let before = batch.len();
            batch.retain(|envelope| envelope.to != id);
            dropped += (before - batch.len()) as u64;
        }
        self.scheduled.retain(|_, batch| !batch.is_empty());
        self.stats.dropped += dropped;
        dropped
    }

    /// Schedules an envelope for a future tick and returns its id.
    pub fn send(&mut self, from: u64, to: u64, latency: u64) -> Result<u64, SimError> {
        if latency == 0 || latency > MAX_LATENCY {
            return Err(SimError::InvalidLatency);
        }
        for id in [from, to] {
            match self.nodes.get(&id) {
                None => return Err(SimError::UnknownNode(id)),
                Some(node) if !node.online => return Err(SimError::NodeOffline(id)),
                Some(_) => {}
            }
        }
        if self.in_flight() >= MAX_INFLIGHT {
            return Err(SimError::QueueFull);
        }
        let id = self.next_envelope;
        self.next_envelope += 1;
        self.stats.sent += 1;
        let envelope = Envelope {
            id,
            from,
            to,
            sent_tick: self.tick,
            deliver_tick: self.tick + latency,
        };
        self.scheduled.entry(envelope.deliver_tick).or_default().push(envelope);
        Ok(id)
    }

    /// Advances one tick and returns how many envelopes were delivered.
    pub fn step(&mut self) -> u64 {
        self.tick += 1;
        let delivered = self.scheduled.remove(&self.tick).map_or(0, Vec::len) as u64;
        self.stats.delivered += delivered;
        delivered
    }

    /// Runs until the target tick and returns the envelopes delivered.
    pub fn run_until(&mut self, target: u64) -> Result<u64, SimError> {
        if target < self.tick {
            return Err(SimError::TimeWarp(self.tick));
        }
        let mut delivered = 0u64;
        while self.tick < target {
            delivered += self.step();
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim(nodes: u64) -> Sim {
        let mut sim = Sim::new(nodes as usize).unwrap();
        for id in 1..=nodes {
            sim.add_node(id).unwrap();
        }
        sim
    }

    #[test]
    fn validates_capacity() {
        assert_eq!(Sim::new(0).unwrap_err(), SimError::InvalidCapacity);
        assert_eq!(
            Sim::new(MAX_NODES + 1).unwrap_err(),
            SimError::InvalidCapacity
        );
        assert_eq!(sim(2).node_count(), 2);
    }

    #[test]
    fn node_lifecycle() {
        let mut s = sim(2);
        assert_eq!(s.add_node(1).unwrap_err(), SimError::NodeExists(1));
        assert_eq!(s.add_node(3).unwrap_err(), SimError::SimFull);
        assert!(s.has_node(1));
        assert_eq!(s.remove_node(1).unwrap(), 0);
        assert!(!s.has_node(1));
        assert_eq!(s.remove_node(1).unwrap_err(), SimError::UnknownNode(1));
        assert_eq!(s.node_count(), 1);
    }

    #[test]
    fn send_validates_endpoints_and_latency() {
        let mut s = sim(2);
        assert_eq!(s.send(1, 3, 1).unwrap_err(), SimError::UnknownNode(3));
        assert_eq!(s.send(3, 1, 1).unwrap_err(), SimError::UnknownNode(3));
        assert_eq!(s.send(1, 2, 0).unwrap_err(), SimError::InvalidLatency);
        assert_eq!(
            s.send(1, 2, MAX_LATENCY + 1).unwrap_err(),
            SimError::InvalidLatency
        );
        assert_eq!(s.send(1, 2, MAX_LATENCY).unwrap(), 1);
        assert_eq!(s.send(1, 2, 1).unwrap(), 2);
    }

    #[test]
    fn delivery_follows_latency() {
        let mut s = sim(2);
        s.send(1, 2, 3).unwrap();
        assert_eq!(s.in_flight(), 1);
        assert_eq!(s.step(), 0);
        assert_eq!(s.step(), 0);
        assert_eq!(s.tick(), 2);
        assert_eq!(s.step(), 1);
        assert_eq!(s.in_flight(), 0);
        let stats = s.stats();
        assert_eq!(stats.sent, 1);
        assert_eq!(stats.delivered, 1);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn run_until_rejects_time_warp() {
        let mut s = sim(2);
        s.send(1, 2, 1).unwrap();
        assert_eq!(s.run_until(5).unwrap(), 1);
        assert_eq!(s.tick(), 5);
        assert_eq!(s.run_until(2).unwrap_err(), SimError::TimeWarp(5));
    }

    #[test]
    fn queue_is_bounded() {
        let mut s = sim(2);
        for _ in 0..MAX_INFLIGHT {
            s.send(1, 2, MAX_LATENCY).unwrap();
        }
        assert_eq!(s.in_flight(), MAX_INFLIGHT);
        assert_eq!(s.send(1, 2, 1).unwrap_err(), SimError::QueueFull);
    }

    #[test]
    fn offline_nodes_block_and_resume() {
        let mut s = sim(2);
        s.send(1, 2, 1).unwrap();
        s.take_offline(2).unwrap();
        assert_eq!(s.send(1, 2, 1).unwrap_err(), SimError::NodeOffline(2));
        assert_eq!(s.stats().dropped, 1);
        s.bring_online(2).unwrap();
        s.send(1, 2, 1).unwrap();
        let target = s.tick() + 1;
        s.run_until(target).unwrap();
        assert_eq!(s.stats().delivered, 1);
    }

    #[test]
    fn remove_node_drops_only_its_inflight() {
        let mut s = sim(2);
        s.send(1, 2, 5).unwrap();
        s.send(2, 1, 5).unwrap();
        assert_eq!(s.remove_node(2).unwrap(), 1);
        assert_eq!(s.stats().dropped, 1);
        assert_eq!(s.in_flight(), 1);
        assert_eq!(
            s.drop_inflight_to(2).unwrap_err(),
            SimError::UnknownNode(2)
        );
    }

    #[test]
    fn identical_runs_are_identical() {
        let run = || {
            let mut s = sim(3);
            for tick in 0..10u64 {
                let from = 1 + (tick % 3);
                let to = 1 + ((tick + 1) % 3);
                s.send(from, to, 1 + (tick % 2)).unwrap();
                s.step();
            }
            (s.tick(), s.stats(), s.in_flight())
        };
        let first = run();
        let second = run();
        assert_eq!(first, second);
    }
}
