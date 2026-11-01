//! Keyed token-bucket rate limiting driven by explicit ticks.
//!
//! Deterministic and replay-friendly: no wall clock, no threads. Callers pass
//! the current tick, so refill behaviour is reproducible from the same
//! request sequence.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_BUCKETS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketConfig {
    pub capacity: u64,
    pub refill_per_tick: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateDecision {
    Allowed { tokens_left: u64 },
    Denied { retry_after_ticks: u64 },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RateLimitError {
    #[error("bucket capacity must be greater than zero")]
    ZeroCapacity,
    #[error("requested amount exceeds bucket capacity")]
    AmountTooLarge,
    #[error("rate limiter is out of keys")]
    TooManyBuckets,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TokenBucket {
    config: BucketConfig,
    tokens: u64,
    last_tick: u64,
}

impl TokenBucket {
    fn new(config: BucketConfig, tick: u64) -> Self {
        Self {
            config,
            tokens: config.capacity,
            last_tick: tick,
        }
    }

    fn advance_to(&mut self, tick: u64) {
        if tick > self.last_tick {
            let elapsed = tick - self.last_tick;
            let gained = elapsed.saturating_mul(self.config.refill_per_tick);
            self.tokens = self.tokens.saturating_add(gained).min(self.config.capacity);
            self.last_tick = tick;
        }
    }
}

/// Keyed token-bucket limiter. Every key gets the same bucket config, so
/// behaviour stays predictable and easy to reason about.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimiter {
    config: BucketConfig,
    buckets: BTreeMap<String, TokenBucket>,
}

impl RateLimiter {
    pub fn new(config: BucketConfig) -> Result<Self, RateLimitError> {
        if config.capacity == 0 {
            return Err(RateLimitError::ZeroCapacity);
        }
        Ok(Self {
            config,
            buckets: BTreeMap::new(),
        })
    }

    pub fn config(&self) -> BucketConfig {
        self.config
    }

    pub fn tracked_keys(&self) -> usize {
        self.buckets.len()
    }

    /// Tries to take `amount` tokens from the key's bucket at the given
    /// tick. New keys start full; denied requests report how many ticks of
    /// refill are needed before the request can succeed.
    pub fn try_acquire(
        &mut self,
        key: &str,
        amount: u64,
        tick: u64,
    ) -> Result<RateDecision, RateLimitError> {
        if amount > self.config.capacity {
            return Err(RateLimitError::AmountTooLarge);
        }
        if !self.buckets.contains_key(key) {
            if self.buckets.len() >= MAX_BUCKETS {
                return Err(RateLimitError::TooManyBuckets);
            }
            let bucket = TokenBucket::new(self.config, tick);
            self.buckets.insert(key.to_string(), bucket);
        }
        let bucket = self.buckets.get_mut(key).expect("bucket was just inserted");
        bucket.advance_to(tick);
        if bucket.tokens >= amount {
            bucket.tokens -= amount;
            Ok(RateDecision::Allowed {
                tokens_left: bucket.tokens,
            })
        } else {
            let needed = amount - bucket.tokens;
            let retry_after_ticks = if self.config.refill_per_tick == 0 {
                u64::MAX
            } else {
                needed.div_ceil(self.config.refill_per_tick)
            };
            Ok(RateDecision::Denied { retry_after_ticks })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_capacity() {
        let err = RateLimiter::new(BucketConfig {
            capacity: 0,
            refill_per_tick: 1,
        });
        assert!(matches!(err, Err(RateLimitError::ZeroCapacity)));
    }

    #[test]
    fn burst_then_deny_then_refill() {
        let mut limiter = RateLimiter::new(BucketConfig {
            capacity: 2,
            refill_per_tick: 1,
        })
        .unwrap();
        assert_eq!(
            limiter.try_acquire("peer", 1, 0),
            Ok(RateDecision::Allowed { tokens_left: 1 })
        );
        assert_eq!(
            limiter.try_acquire("peer", 1, 0),
            Ok(RateDecision::Allowed { tokens_left: 0 })
        );
        assert_eq!(
            limiter.try_acquire("peer", 1, 0),
            Ok(RateDecision::Denied {
                retry_after_ticks: 1
            })
        );
        assert_eq!(
            limiter.try_acquire("peer", 1, 3),
            Ok(RateDecision::Allowed { tokens_left: 1 })
        );
    }

    #[test]
    fn keys_are_independent() {
        let mut limiter = RateLimiter::new(BucketConfig {
            capacity: 1,
            refill_per_tick: 0,
        })
        .unwrap();
        assert_eq!(
            limiter.try_acquire("a", 1, 0),
            Ok(RateDecision::Allowed { tokens_left: 0 })
        );
        assert_eq!(
            limiter.try_acquire("b", 1, 0),
            Ok(RateDecision::Allowed { tokens_left: 0 })
        );
        assert_eq!(limiter.tracked_keys(), 2);
    }

    #[test]
    fn zero_refill_never_recovers() {
        let mut limiter = RateLimiter::new(BucketConfig {
            capacity: 1,
            refill_per_tick: 0,
        })
        .unwrap();
        assert_eq!(
            limiter.try_acquire("a", 1, 0),
            Ok(RateDecision::Allowed { tokens_left: 0 })
        );
        assert_eq!(
            limiter.try_acquire("a", 1, 100),
            Ok(RateDecision::Denied {
                retry_after_ticks: u64::MAX
            })
        );
    }

    #[test]
    fn amount_above_capacity_is_invalid() {
        let mut limiter = RateLimiter::new(BucketConfig {
            capacity: 1,
            refill_per_tick: 1,
        })
        .unwrap();
        assert_eq!(
            limiter.try_acquire("a", 2, 0),
            Err(RateLimitError::AmountTooLarge)
        );
    }
}
