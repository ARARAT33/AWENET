//! Deterministic circuit breaker with explicit-tick cooldown.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. The breaker
//! moves between Closed, Open and HalfOpen states: repeated failures open
//! the circuit, the cooldown gates a single probe, and the probe outcome
//! closes the circuit or opens it again.

use thiserror::Error;

pub const MAX_FAILURE_THRESHOLD: u32 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CircuitError {
    #[error("failure threshold must be greater than zero")]
    ZeroThreshold,
    #[error("failure threshold exceeds the maximum of {MAX_FAILURE_THRESHOLD}")]
    ThresholdTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { retry_after_ticks: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Opened { at_tick: u64 },
    Closed { at_tick: u64 },
}

#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown_ticks: u64,
    state: State,
    consecutive_failures: u32,
    opened_at: u64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown_ticks: u64) -> Result<Self, CircuitError> {
        if failure_threshold == 0 {
            return Err(CircuitError::ZeroThreshold);
        }
        if failure_threshold > MAX_FAILURE_THRESHOLD {
            return Err(CircuitError::ThresholdTooLarge);
        }
        Ok(Self {
            failure_threshold,
            cooldown_ticks,
            state: State::Closed,
            consecutive_failures: 0,
            opened_at: 0,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Decides whether a call may proceed at `now`. Once the cooldown has
    /// elapsed, an Open circuit becomes HalfOpen and allows one probe.
    pub fn allow(&mut self, now: u64) -> Decision {
        if self.state == State::Open {
            let elapsed = now.saturating_sub(self.opened_at);
            if elapsed >= self.cooldown_ticks {
                self.state = State::HalfOpen;
                Decision::Allow
            } else {
                Decision::Deny {
                    retry_after_ticks: self.cooldown_ticks - elapsed,
                }
            }
        } else {
            Decision::Allow
        }
    }

    pub fn record_success(&mut self, now: u64) -> Option<Transition> {
        match self.state {
            State::HalfOpen => {
                self.state = State::Closed;
                self.consecutive_failures = 0;
                Some(Transition::Closed { at_tick: now })
            }
            State::Closed => {
                self.consecutive_failures = 0;
                None
            }
            State::Open => None,
        }
    }

    pub fn record_failure(&mut self, now: u64) -> Option<Transition> {
        match self.state {
            State::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.failure_threshold {
                    self.state = State::Open;
                    self.consecutive_failures = 0;
                    self.opened_at = now;
                    return Some(Transition::Opened { at_tick: now });
                }
                None
            }
            State::HalfOpen => {
                self.state = State::Open;
                self.opened_at = now;
                Some(Transition::Opened { at_tick: now })
            }
            State::Open => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_threshold() {
        assert_eq!(
            CircuitBreaker::new(0, 10).unwrap_err(),
            CircuitError::ZeroThreshold
        );
        assert_eq!(
            CircuitBreaker::new(MAX_FAILURE_THRESHOLD + 1, 10).unwrap_err(),
            CircuitError::ThresholdTooLarge
        );
        assert!(CircuitBreaker::new(3, 10).is_ok());
    }

    #[test]
    fn opens_after_consecutive_failures() {
        let mut breaker = CircuitBreaker::new(3, 10).unwrap();
        assert_eq!(breaker.state(), State::Closed);
        assert_eq!(breaker.record_failure(1), None);
        assert_eq!(breaker.record_failure(2), None);
        assert_eq!(
            breaker.record_failure(3),
            Some(Transition::Opened { at_tick: 3 })
        );
        assert_eq!(breaker.state(), State::Open);
    }

    #[test]
    fn success_resets_failure_count() {
        let mut breaker = CircuitBreaker::new(3, 10).unwrap();
        breaker.record_failure(1);
        breaker.record_failure(2);
        breaker.record_success(3);
        assert_eq!(breaker.consecutive_failures(), 0);
        assert_eq!(breaker.record_failure(4), None);
        assert_eq!(breaker.record_failure(5), None);
        assert_eq!(breaker.state(), State::Closed);
    }

    #[test]
    fn deny_reports_retry_after() {
        let mut breaker = CircuitBreaker::new(1, 10).unwrap();
        breaker.record_failure(5);
        assert_eq!(
            breaker.allow(8),
            Decision::Deny {
                retry_after_ticks: 7
            }
        );
        assert_eq!(breaker.state(), State::Open);
    }

    #[test]
    fn half_open_probe_cycle() {
        let mut breaker = CircuitBreaker::new(1, 10).unwrap();
        breaker.record_failure(0);
        // Cooldown elapsed: the next allow is the probe.
        assert_eq!(breaker.allow(10), Decision::Allow);
        assert_eq!(breaker.state(), State::HalfOpen);
        assert_eq!(
            breaker.record_success(11),
            Some(Transition::Closed { at_tick: 11 })
        );
        assert_eq!(breaker.state(), State::Closed);

        // A failed probe re-opens the circuit with a fresh cooldown.
        breaker.record_failure(20);
        assert_eq!(breaker.allow(30), Decision::Allow);
        assert_eq!(
            breaker.record_failure(31),
            Some(Transition::Opened { at_tick: 31 })
        );
        assert_eq!(breaker.state(), State::Open);
        assert_eq!(
            breaker.allow(35),
            Decision::Deny {
                retry_after_ticks: 6
            }
        );
    }
}
