//! Bounded node event log for audit and observability.
//! Keeps a deterministic, replay-friendly history of node events with a
//! hard capacity bound so observability never grows memory without limit.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const DEFAULT_EVENT_CAPACITY: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum NodeEvent {
    NodeStarted,
    PeerDiscovered { peer: String },
    PeerLost { peer: String },
    TransferSucceeded { resource: String, bytes: u64 },
    TransferFailed { resource: String, reason: String },
    RepairCompleted { shard: u16, replicas: u16 },
    HealthScoreChanged { from: u32, to: u32 },
    Custom { message: String },
}

impl NodeEvent {
    pub fn label(&self) -> &'static str {
        match self {
            NodeEvent::NodeStarted => "node_started",
            NodeEvent::PeerDiscovered { .. } => "peer_discovered",
            NodeEvent::PeerLost { .. } => "peer_lost",
            NodeEvent::TransferSucceeded { .. } => "transfer_succeeded",
            NodeEvent::TransferFailed { .. } => "transfer_failed",
            NodeEvent::RepairCompleted { .. } => "repair_completed",
            NodeEvent::HealthScoreChanged { .. } => "health_score_changed",
            NodeEvent::Custom { .. } => "custom",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimestampedEvent {
    pub tick: u64,
    pub event: NodeEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventLog {
    capacity: usize,
    events: VecDeque<TimestampedEvent>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_CAPACITY)
    }
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Records an event at the given tick. The oldest entry is dropped once
    /// the configured capacity is reached.
    pub fn record(&mut self, tick: u64, event: NodeEvent) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(TimestampedEvent { tick, event });
    }

    pub fn iter(&self) -> impl Iterator<Item = &TimestampedEvent> {
        self.events.iter()
    }

    /// All events recorded at or after the given tick.
    pub fn since(&self, tick: u64) -> Vec<&TimestampedEvent> {
        self.events.iter().filter(|e| e.tick >= tick).collect()
    }

    pub fn count_with_label(&self, label: &str) -> usize {
        self.events
            .iter()
            .filter(|e| e.event.label() == label)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_bounded() {
        let mut log = EventLog::new(4);
        for i in 0..10u64 {
            log.record(i, NodeEvent::NodeStarted);
        }
        assert_eq!(log.len(), 4);
        assert_eq!(log.capacity(), 4);
        let ticks: Vec<u64> = log.iter().map(|e| e.tick).collect();
        assert_eq!(ticks, vec![6, 7, 8, 9]);
    }

    #[test]
    fn filters_and_counts() {
        let mut log = EventLog::new(16);
        log.record(1, NodeEvent::NodeStarted);
        log.record(2, NodeEvent::PeerDiscovered { peer: "n1".into() });
        log.record(
            3,
            NodeEvent::TransferFailed {
                resource: "r1".into(),
                reason: "timeout".into(),
            },
        );
        assert_eq!(log.since(2).len(), 2);
        assert_eq!(log.count_with_label("peer_discovered"), 1);
        assert_eq!(log.count_with_label("node_started"), 1);
        assert_eq!(log.count_with_label("custom"), 0);
    }

    #[test]
    fn serializes_to_json() {
        let mut log = EventLog::new(8);
        log.record(1, NodeEvent::HealthScoreChanged { from: 90, to: 75 });
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("health_score_changed"));
        let back: EventLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
    }
}
