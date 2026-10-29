//! Deterministic maintenance scheduler.
//! Tick-driven job scheduling for node maintenance work (repair sweeps,
//! backups, health recomputation). No wall clock and no threads, so runs
//! are replayable and testable.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("job name is empty")]
    EmptyName,
    #[error("interval must be at least one tick")]
    ZeroInterval,
    #[error("job already exists: {0}")]
    Duplicate(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceJob {
    pub name: String,
    pub interval_ticks: u64,
    pub next_run: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scheduler {
    jobs: Vec<MaintenanceJob>,
    tick: u64,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn jobs(&self) -> &[MaintenanceJob] {
        &self.jobs
    }

    pub fn add_job(&mut self, name: &str, interval_ticks: u64) -> Result<(), SchedulerError> {
        if name.is_empty() {
            return Err(SchedulerError::EmptyName);
        }
        if interval_ticks == 0 {
            return Err(SchedulerError::ZeroInterval);
        }
        if self.jobs.iter().any(|j| j.name == name) {
            return Err(SchedulerError::Duplicate(name.to_string()));
        }
        self.jobs.push(MaintenanceJob {
            name: name.to_string(),
            interval_ticks,
            next_run: self.tick,
        });
        Ok(())
    }

    /// Advances the scheduler by one tick and returns the names of the jobs
    /// that are due to run on this tick, in registration order.
    pub fn advance(&mut self) -> Vec<String> {
        self.tick += 1;
        let tick = self.tick;
        self.jobs
            .iter_mut()
            .filter(|j| tick >= j.next_run)
            .map(|j| {
                let name = j.name.clone();
                j.next_run = tick + j.interval_ticks;
                name
            })
            .collect()
    }

    /// Number of ticks until the next job is due (0 when a job is overdue).
    pub fn ticks_until_next(&self) -> Option<u64> {
        self.jobs
            .iter()
            .map(|j| j.next_run.saturating_sub(self.tick))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_recurring_jobs() {
        let mut s = Scheduler::new();
        s.add_job("repair_sweep", 3).unwrap();
        s.add_job("health_check", 2).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.ticks_until_next(), Some(0));

        let t1 = s.advance();
        assert_eq!(t1, vec!["repair_sweep", "health_check"]);
        let t2 = s.advance();
        assert!(t2.is_empty());
        let t3 = s.advance();
        assert_eq!(t3, vec!["health_check"]);
        let t4 = s.advance();
        assert_eq!(t4, vec!["repair_sweep", "health_check"]);
    }

    #[test]
    fn rejects_bad_jobs() {
        let mut s = Scheduler::new();
        assert_eq!(s.add_job("", 5), Err(SchedulerError::EmptyName));
        assert_eq!(s.add_job("x", 0), Err(SchedulerError::ZeroInterval));
        s.add_job("x", 5).unwrap();
        assert_eq!(
            s.add_job("x", 7),
            Err(SchedulerError::Duplicate("x".into()))
        );
        assert_eq!(s.ticks_until_next(), None);
        assert!(!s.is_empty());
    }
}
