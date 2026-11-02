//! Bounded write-ahead operation journal with sequence numbers.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Operations append as monotonically numbered records; a
//! checkpoint truncates everything up to a sequence so recovery replays
//! only the suffix that is not yet durable.

use std::collections::VecDeque;
use thiserror::Error;

pub const MAX_CAPACITY: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum JournalError {
    #[error("journal capacity must be greater than zero")]
    ZeroCapacity,
    #[error("journal capacity exceeds the maximum of {MAX_CAPACITY}")]
    CapacityTooLarge,
    #[error("journal is full; checkpoint before appending")]
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub kind: String,
    pub data: String,
}

#[derive(Debug)]
pub struct Journal {
    capacity: usize,
    records: VecDeque<Record>,
    next_seq: u64,
    checkpoint_seq: u64,
}

impl Journal {
    pub fn new(capacity: usize) -> Result<Self, JournalError> {
        if capacity == 0 {
            return Err(JournalError::ZeroCapacity);
        }
        if capacity > MAX_CAPACITY {
            return Err(JournalError::CapacityTooLarge);
        }
        Ok(Self {
            capacity,
            records: VecDeque::new(),
            next_seq: 0,
            checkpoint_seq: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    pub fn append(&mut self, kind: &str, data: &str) -> Result<u64, JournalError> {
        if self.records.len() >= self.capacity {
            return Err(JournalError::Full);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.records.push_back(Record {
            seq,
            kind: kind.to_string(),
            data: data.to_string(),
        });
        Ok(seq)
    }

    /// Replays every retained record in sequence order.
    pub fn replay<F: FnMut(&Record)>(&self, mut apply: F) {
        for record in &self.records {
            apply(record);
        }
    }

    /// Returns the retained records appended after `seq`.
    pub fn records_after(&self, seq: u64) -> Vec<Record> {
        self.records
            .iter()
            .filter(|r| r.seq > seq)
            .cloned()
            .collect()
    }

    /// Marks everything up to `seq` as durable and truncates it.
    /// Returns the number of truncated records.
    pub fn checkpoint(&mut self, seq: u64) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.seq > seq);
        self.checkpoint_seq = self.checkpoint_seq.max(seq);
        before - self.records.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_capacity() {
        assert_eq!(
            Journal::new(0).unwrap_err(),
            JournalError::ZeroCapacity
        );
        assert_eq!(
            Journal::new(MAX_CAPACITY + 1).unwrap_err(),
            JournalError::CapacityTooLarge
        );
        assert!(Journal::new(1).is_ok());
    }

    #[test]
    fn sequences_are_monotonic() {
        let mut journal = Journal::new(8).unwrap();
        assert!(journal.is_empty());
        assert_eq!(journal.last_seq(), 0);
        assert_eq!(journal.append("put", "a").unwrap(), 0);
        assert_eq!(journal.append("put", "b").unwrap(), 1);
        assert_eq!(journal.append("del", "c").unwrap(), 2);
        assert_eq!(journal.len(), 3);
        assert_eq!(journal.last_seq(), 2);
    }

    #[test]
    fn full_journal_rejects_appends() {
        let mut journal = Journal::new(2).unwrap();
        journal.append("put", "a").unwrap();
        journal.append("put", "b").unwrap();
        assert_eq!(journal.append("put", "c").unwrap_err(), JournalError::Full);
    }

    #[test]
    fn replay_preserves_order() {
        let mut journal = Journal::new(8).unwrap();
        journal.append("put", "a").unwrap();
        journal.append("put", "b").unwrap();
        journal.append("del", "c").unwrap();
        let mut seen = Vec::new();
        journal.replay(|r| seen.push(r.kind.clone()));
        assert_eq!(seen, vec!["put", "put", "del"]);
    }

    #[test]
    fn checkpoint_truncates_suffix_recovery() {
        let mut journal = Journal::new(8).unwrap();
        let s0 = journal.append("put", "a").unwrap();
        journal.append("put", "b").unwrap();
        journal.append("del", "c").unwrap();
        assert_eq!(journal.checkpoint(s0), 1);
        assert_eq!(journal.checkpoint_seq(), s0);
        assert_eq!(journal.len(), 2);
        let suffix = journal.records_after(s0);
        assert_eq!(suffix.len(), 2);
        assert_eq!(suffix[0].kind, "put");
        assert_eq!(suffix[1].kind, "del");
        // Appends keep working after a checkpoint freed capacity.
        assert!(journal.append("put", "d").is_ok());
    }
}
