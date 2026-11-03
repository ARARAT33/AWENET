//! Subscription lifecycle with trials, renewals and grace periods.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. Subscriptions
//! move through Trialing, Active, PastDue, Cancelled and Expired states;
//! `advance` applies the lifecycle transitions for a tick.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_SUBSCRIPTIONS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SubscriptionError {
    #[error("subscription capacity must be between 1 and {MAX_SUBSCRIPTIONS}")]
    InvalidCapacity,
    #[error("subscription registry is full (max {MAX_SUBSCRIPTIONS})")]
    RegistryFull,
    #[error("plan period must be greater than zero ticks")]
    InvalidPeriod,
    #[error("unknown subscription {0}")]
    UnknownSubscription(u64),
    #[error("subscription {0} is in state {1:?}")]
    InvalidState(u64, SubscriptionState),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub name: String,
    pub period_ticks: u64,
    pub grace_ticks: u64,
    pub price: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionState {
    Trialing,
    Active,
    PastDue,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug)]
pub struct Subscription {
    pub id: u64,
    pub subscriber: String,
    pub plan: Plan,
    pub state: SubscriptionState,
    pub current_period_end: u64,
    pub trial_end: Option<u64>,
}

#[derive(Debug)]
pub struct Subscriptions {
    max_subscriptions: usize,
    next_id: u64,
    subscriptions: BTreeMap<u64, Subscription>,
}

impl Subscriptions {
    pub fn new(max_subscriptions: usize) -> Result<Self, SubscriptionError> {
        if max_subscriptions == 0 || max_subscriptions > MAX_SUBSCRIPTIONS {
            return Err(SubscriptionError::InvalidCapacity);
        }
        Ok(Self {
            max_subscriptions,
            next_id: 1,
            subscriptions: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Subscription> {
        self.subscriptions.get(&id)
    }

    /// Starts a subscription, optionally inside a trial window.
    pub fn subscribe(
        &mut self,
        subscriber: &str,
        plan: Plan,
        now: u64,
        trial_ticks: Option<u64>,
    ) -> Result<u64, SubscriptionError> {
        if plan.period_ticks == 0 {
            return Err(SubscriptionError::InvalidPeriod);
        }
        if self.subscriptions.len() >= self.max_subscriptions {
            return Err(SubscriptionError::RegistryFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        let (state, current_period_end, trial_end) = match trial_ticks {
            Some(trial) => (SubscriptionState::Trialing, now + trial, Some(now + trial)),
            None => (SubscriptionState::Active, now + plan.period_ticks, None),
        };
        self.subscriptions.insert(
            id,
            Subscription {
                id,
                subscriber: subscriber.to_string(),
                plan,
                state,
                current_period_end,
                trial_end,
            },
        );
        Ok(id)
    }

    /// Converts a trialing subscription into an active one for a full
    /// period from `now`.
    pub fn activate(&mut self, id: u64, now: u64) -> Result<(), SubscriptionError> {
        let subscription = self
            .subscriptions
            .get_mut(&id)
            .ok_or(SubscriptionError::UnknownSubscription(id))?;
        if subscription.state != SubscriptionState::Trialing {
            return Err(SubscriptionError::InvalidState(id, subscription.state));
        }
        subscription.state = SubscriptionState::Active;
        subscription.current_period_end = now + subscription.plan.period_ticks;
        Ok(())
    }

    /// Renews a trialing or active subscription for one period from
    /// `now`. Past-due subscriptions must recover via `advance` or stay
    /// cancelled.
    pub fn renew(&mut self, id: u64, now: u64) -> Result<u64, SubscriptionError> {
        let subscription = self
            .subscriptions
            .get_mut(&id)
            .ok_or(SubscriptionError::UnknownSubscription(id))?;
        match subscription.state {
            SubscriptionState::Trialing | SubscriptionState::Active => {
                subscription.state = SubscriptionState::Active;
                subscription.current_period_end = now + subscription.plan.period_ticks;
                Ok(subscription.current_period_end)
            }
            state => Err(SubscriptionError::InvalidState(id, state)),
        }
    }

    pub fn cancel(&mut self, id: u64) -> Result<(), SubscriptionError> {
        let subscription = self
            .subscriptions
            .get_mut(&id)
            .ok_or(SubscriptionError::UnknownSubscription(id))?;
        match subscription.state {
            SubscriptionState::Trialing
            | SubscriptionState::Active
            | SubscriptionState::PastDue => {
                subscription.state = SubscriptionState::Cancelled;
                Ok(())
            }
            state => Err(SubscriptionError::InvalidState(id, state)),
        }
    }

    /// Applies lifecycle transitions at `now` and returns the transitions
    /// as (id, new state) pairs: lapsed trials and exhausted grace expire,
    /// and active subscriptions whose period ended move to PastDue.
    pub fn advance(&mut self, now: u64) -> Vec<(u64, SubscriptionState)> {
        let mut transitions = Vec::new();
        for subscription in self.subscriptions.values_mut() {
            match subscription.state {
                SubscriptionState::Trialing => {
                    if subscription.trial_end.is_some_and(|end| end <= now) {
                        subscription.state = SubscriptionState::Expired;
                        transitions.push((subscription.id, SubscriptionState::Expired));
                    }
                }
                SubscriptionState::Active => {
                    if subscription.current_period_end <= now {
                        subscription.state = SubscriptionState::PastDue;
                        transitions.push((subscription.id, SubscriptionState::PastDue));
                    }
                }
                SubscriptionState::PastDue => {
                    let grace_end = subscription
                        .current_period_end
                        .saturating_add(subscription.plan.grace_ticks);
                    if grace_end <= now {
                        subscription.state = SubscriptionState::Expired;
                        transitions.push((subscription.id, SubscriptionState::Expired));
                    }
                }
                SubscriptionState::Cancelled | SubscriptionState::Expired => {}
            }
        }
        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(period: u64, grace: u64) -> Plan {
        Plan {
            name: "awe-plus".into(),
            period_ticks: period,
            grace_ticks: grace,
            price: 1000,
        }
    }

    #[test]
    fn validates_capacity_and_period() {
        assert_eq!(
            Subscriptions::new(0).unwrap_err(),
            SubscriptionError::InvalidCapacity
        );
        assert_eq!(
            Subscriptions::new(MAX_SUBSCRIPTIONS + 1).unwrap_err(),
            SubscriptionError::InvalidCapacity
        );
        let mut subs = Subscriptions::new(4).unwrap();
        assert_eq!(
            subs.subscribe("alice", plan(0, 0), 0, None).unwrap_err(),
            SubscriptionError::InvalidPeriod
        );
    }

    #[test]
    fn registry_full_is_rejected() {
        let mut subs = Subscriptions::new(1).unwrap();
        subs.subscribe("alice", plan(30, 5), 0, None).unwrap();
        assert_eq!(
            subs.subscribe("bob", plan(30, 5), 0, None).unwrap_err(),
            SubscriptionError::RegistryFull
        );
        assert_eq!(subs.len(), 1);
        assert!(!subs.is_empty());
    }

    #[test]
    fn trial_activation_and_lapse() {
        let mut subs = Subscriptions::new(4).unwrap();
        let id = subs.subscribe("alice", plan(30, 5), 0, Some(10)).unwrap();
        assert_eq!(subs.get(id).unwrap().state, SubscriptionState::Trialing);
        assert_eq!(subs.activate(id, 5).unwrap(), ());
        assert_eq!(subs.get(id).unwrap().state, SubscriptionState::Active);
        assert_eq!(subs.get(id).unwrap().current_period_end, 35);

        let id2 = subs.subscribe("bob", plan(30, 5), 0, Some(10)).unwrap();
        assert_eq!(
            subs.advance(10),
            vec![(id2, SubscriptionState::Expired)]
        );
        assert_eq!(subs.get(id2).unwrap().state, SubscriptionState::Expired);
    }

    #[test]
    fn renewal_extends_the_period() {
        let mut subs = Subscriptions::new(4).unwrap();
        let id = subs.subscribe("alice", plan(30, 5), 0, None).unwrap();
        assert_eq!(subs.renew(id, 20).unwrap(), 50);
        assert_eq!(subs.get(id).unwrap().state, SubscriptionState::Active);
        assert_eq!(subs.advance(49).len(), 0);
        assert_eq!(
            subs.advance(50),
            vec![(id, SubscriptionState::PastDue)]
        );
        assert_eq!(
            subs.renew(id, 55).unwrap_err(),
            SubscriptionError::InvalidState(id, SubscriptionState::PastDue)
        );
    }

    #[test]
    fn grace_expires_and_cancel_stops_the_clock() {
        let mut subs = Subscriptions::new(4).unwrap();
        let id = subs.subscribe("alice", plan(10, 5), 0, None).unwrap();
        subs.advance(10);
        assert_eq!(subs.get(id).unwrap().state, SubscriptionState::PastDue);
        assert_eq!(subs.advance(15).len(), 0);
        assert_eq!(
            subs.advance(15 + 1),
            vec![(id, SubscriptionState::Expired)]
        );

        let id2 = subs.subscribe("bob", plan(10, 5), 0, None).unwrap();
        subs.cancel(id2).unwrap();
        assert_eq!(
            subs.cancel(id2).unwrap_err(),
            SubscriptionError::InvalidState(id2, SubscriptionState::Cancelled)
        );
        assert!(subs.advance(1000).is_empty());
        assert_eq!(
            subs.activate(99, 0).unwrap_err(),
            SubscriptionError::UnknownSubscription(99)
        );
    }
}
