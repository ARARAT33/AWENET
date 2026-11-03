//! Bounded webhook registry with cooldowns and dead-lettering.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. `plan`
//! produces delivery plans per event: ready attempts, cooldown deferrals
//! with `retry_after_ticks`, and dead letters once a hook exhausts its
//! attempt budget.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_WEBHOOKS: usize = 1_000;
pub const MAX_DELIVERY_ATTEMPTS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum WebhookError {
    #[error("webhook registry capacity must be between 1 and {MAX_WEBHOOKS}")]
    InvalidCapacity,
    #[error("webhook registry is full (max {MAX_WEBHOOKS} hooks)")]
    RegistryFull,
    #[error("max attempts must be between 1 and {MAX_DELIVERY_ATTEMPTS}")]
    InvalidMaxAttempts,
    #[error("unknown webhook {0}")]
    UnknownWebhook(u64),
}

#[derive(Clone, Debug)]
pub struct Webhook {
    pub id: u64,
    pub url: String,
    pub events: Vec<String>,
    pub max_attempts: u32,
    pub cooldown_ticks: u64,
    pub failures: u32,
    pub delivered: u64,
    pub dead: bool,
    pub last_failure_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    Ready { attempt: u32 },
    Cooldown { retry_after_ticks: u64 },
    Dead,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedDelivery {
    pub webhook_id: u64,
    pub url: String,
    pub event: String,
    pub status: DeliveryStatus,
}

#[derive(Debug)]
pub struct Webhooks {
    max_hooks: usize,
    next_id: u64,
    hooks: BTreeMap<u64, Webhook>,
}

impl Webhooks {
    pub fn new(max_hooks: usize) -> Result<Self, WebhookError> {
        if max_hooks == 0 || max_hooks > MAX_WEBHOOKS {
            return Err(WebhookError::InvalidCapacity);
        }
        Ok(Self {
            max_hooks,
            next_id: 1,
            hooks: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Webhook> {
        self.hooks.get(&id)
    }

    /// Registers a hook listening on the given event kinds.
    pub fn register(
        &mut self,
        url: &str,
        events: Vec<String>,
        max_attempts: u32,
        cooldown_ticks: u64,
    ) -> Result<u64, WebhookError> {
        if max_attempts == 0 || max_attempts > MAX_DELIVERY_ATTEMPTS {
            return Err(WebhookError::InvalidMaxAttempts);
        }
        if self.hooks.len() >= self.max_hooks {
            return Err(WebhookError::RegistryFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.hooks.insert(
            id,
            Webhook {
                id,
                url: url.to_string(),
                events,
                max_attempts,
                cooldown_ticks,
                failures: 0,
                delivered: 0,
                dead: false,
                last_failure_tick: 0,
            },
        );
        Ok(id)
    }

    /// Plans deliveries for an event at `now`. Hooks in cooldown defer with
    /// a retry hint; dead hooks are reported as dead letters.
    pub fn plan(&self, event: &str, now: u64) -> Vec<PlannedDelivery> {
        self.hooks
            .values()
            .filter(|hook| !hook.dead && hook.events.iter().any(|kind| kind == event))
            .map(|hook| {
                let status = if hook.failures > 0 {
                    let elapsed = now.saturating_sub(hook.last_failure_tick);
                    if elapsed < hook.cooldown_ticks {
                        DeliveryStatus::Cooldown {
                            retry_after_ticks: hook.cooldown_ticks - elapsed,
                        }
                    } else {
                        DeliveryStatus::Ready {
                            attempt: hook.failures + 1,
                        }
                    }
                } else {
                    DeliveryStatus::Ready { attempt: 1 }
                };
                PlannedDelivery {
                    webhook_id: hook.id,
                    url: hook.url.clone(),
                    event: event.to_string(),
                    status,
                }
            })
            .collect()
    }

    /// Records a successful delivery and clears the failure streak.
    pub fn record_success(&mut self, id: u64) -> Result<(), WebhookError> {
        let hook = self
            .hooks
            .get_mut(&id)
            .ok_or(WebhookError::UnknownWebhook(id))?;
        hook.failures = 0;
        hook.delivered += 1;
        Ok(())
    }

    /// Records a failed delivery; the hook dead-letters once the attempt
    /// budget is exhausted.
    pub fn record_failure(&mut self, id: u64, now: u64) -> Result<(), WebhookError> {
        let hook = self
            .hooks
            .get_mut(&id)
            .ok_or(WebhookError::UnknownWebhook(id))?;
        hook.failures += 1;
        hook.last_failure_tick = now;
        if hook.failures >= hook.max_attempts {
            hook.dead = true;
        }
        Ok(())
    }

    /// Lists dead-lettered hook ids in registration order.
    pub fn dead_letters(&self) -> Vec<u64> {
        self.hooks
            .values()
            .filter(|hook| hook.dead)
            .map(|hook| hook.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<String> {
        vec!["storage.put".into()]
    }

    #[test]
    fn validates_capacity_and_attempts() {
        assert_eq!(Webhooks::new(0).unwrap_err(), WebhookError::InvalidCapacity);
        assert_eq!(
            Webhooks::new(MAX_WEBHOOKS + 1).unwrap_err(),
            WebhookError::InvalidCapacity
        );
        let mut hooks = Webhooks::new(4).unwrap();
        assert_eq!(
            hooks.register("u", kinds(), 0, 1).unwrap_err(),
            WebhookError::InvalidMaxAttempts
        );
    }

    #[test]
    fn registry_full_is_rejected() {
        let mut hooks = Webhooks::new(1).unwrap();
        hooks.register("https://a", kinds(), 3, 2).unwrap();
        assert_eq!(
            hooks.register("https://b", kinds(), 3, 2).unwrap_err(),
            WebhookError::RegistryFull
        );
    }

    #[test]
    fn plan_filters_by_event_and_reports_first_attempt() {
        let mut hooks = Webhooks::new(4).unwrap();
        hooks.register("https://a", kinds(), 3, 2).unwrap();
        hooks
            .register("https://b", vec!["messenger.send".into()], 3, 2)
            .unwrap();
        let plan = hooks.plan("storage.put", 0);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].url, "https://a");
        assert_eq!(plan[0].status, DeliveryStatus::Ready { attempt: 1 });
        assert!(hooks.plan("unknown.event", 0).is_empty());
    }

    #[test]
    fn cooldown_defers_with_retry_hint() {
        let mut hooks = Webhooks::new(4).unwrap();
        let id = hooks.register("https://a", kinds(), 3, 10).unwrap();
        hooks.record_failure(id, 5).unwrap();
        let plan = hooks.plan("storage.put", 8);
        assert_eq!(
            plan[0].status,
            DeliveryStatus::Cooldown {
                retry_after_ticks: 7
            }
        );
        let plan = hooks.plan("storage.put", 15);
        assert_eq!(plan[0].status, DeliveryStatus::Ready { attempt: 2 });
    }

    #[test]
    fn success_resets_failure_streak() {
        let mut hooks = Webhooks::new(4).unwrap();
        let id = hooks.register("https://a", kinds(), 3, 2).unwrap();
        hooks.record_failure(id, 1).unwrap();
        hooks.record_success(id).unwrap();
        let plan = hooks.plan("storage.put", 2);
        assert_eq!(plan[0].status, DeliveryStatus::Ready { attempt: 1 });
        assert_eq!(hooks.get(id).unwrap().delivered, 1);
    }

    #[test]
    fn exhausted_budget_dead_letters() {
        let mut hooks = Webhooks::new(4).unwrap();
        let id = hooks.register("https://a", kinds(), 2, 2).unwrap();
        hooks.record_failure(id, 1).unwrap();
        hooks.record_failure(id, 3).unwrap();
        assert_eq!(hooks.dead_letters(), vec![id]);
        assert!(hooks.plan("storage.put", 10).is_empty());
        assert_eq!(
            hooks.record_success(99).unwrap_err(),
            WebhookError::UnknownWebhook(99)
        );
        assert_eq!(hooks.len(), 1);
        assert!(!hooks.is_empty());
    }
}
