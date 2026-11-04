//! Founder-only official ONECOIN price control with a full change history.
//!
//! Sellers in the AWENET Store price their own products in ONECOIN; that is a
//! product price. The official ONECOIN price is a different quantity and only
//! the founder may change it. Every change is recorded with the old price,
//! the new price, an effective time, a public reason and the founder
//! identity, and the history is append-only.

use crate::founder::{FounderAction, FounderAuthority, FounderError};
use crate::identity::AweId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MIN_PRICE_USD_CENTS: u64 = 1;
pub const MAX_REASON_CHARS: usize = 256;

pub const PRICE_DISCLAIMER: &str = "The official internal ONECOIN price is a network policy reference. It does not guarantee exchange at the same value on external markets or redemption for real money.";

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PriceControlError {
    #[error(transparent)]
    Founder(#[from] FounderError),
    #[error("official price must be at least {MIN_PRICE_USD_CENTS} cent")]
    InvalidPrice,
    #[error("a public reason is required and must not exceed {MAX_REASON_CHARS} characters")]
    InvalidReason,
    #[error("a price change must not take effect before the previous one")]
    NonMonotonicEffectiveTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceChange {
    pub sequence: u64,
    pub old_price_usd_cents: u64,
    pub new_price_usd_cents: u64,
    pub effective_at_unix: u64,
    pub recorded_at_unix: u64,
    pub reason: String,
    pub changed_by: AweId,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfficialPriceControl {
    initial_price_usd_cents: u64,
    history: Vec<PriceChange>,
}

impl OfficialPriceControl {
    pub fn new(initial_price_usd_cents: u64) -> Result<Self, PriceControlError> {
        if initial_price_usd_cents < MIN_PRICE_USD_CENTS {
            return Err(PriceControlError::InvalidPrice);
        }
        Ok(Self {
            initial_price_usd_cents,
            history: Vec::new(),
        })
    }

    /// The official price in force at `now_unix`.
    pub fn price_at(&self, now_unix: u64) -> u64 {
        let mut price = self.initial_price_usd_cents;
        for change in &self.history {
            if change.effective_at_unix <= now_unix {
                price = change.new_price_usd_cents;
            }
        }
        price
    }

    pub fn current_price(&self) -> u64 {
        self.price_at(u64::MAX)
    }

    pub fn history(&self) -> &[PriceChange] {
        &self.history
    }

    /// Sets a new official price. Only the founder key stored in the founder
    /// authority may call this; every other caller is rejected.
    pub fn set_price(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        new_price_usd_cents: u64,
        effective_at_unix: u64,
        recorded_at_unix: u64,
        reason: &str,
    ) -> Result<&PriceChange, PriceControlError> {
        if new_price_usd_cents < MIN_PRICE_USD_CENTS {
            return Err(PriceControlError::InvalidPrice);
        }
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > MAX_REASON_CHARS {
            return Err(PriceControlError::InvalidReason);
        }
        if let Some(last) = self.history.last() {
            if effective_at_unix < last.effective_at_unix {
                return Err(PriceControlError::NonMonotonicEffectiveTime);
            }
        }
        authority.authorize(
            actor_public_key,
            FounderAction::SetOfficialPrice,
            recorded_at_unix,
            "set official ONECOIN price",
        )?;
        let change = PriceChange {
            sequence: self.history.len() as u64 + 1,
            old_price_usd_cents: self.current_price(),
            new_price_usd_cents,
            effective_at_unix,
            recorded_at_unix,
            reason: reason.to_string(),
            changed_by: authority.founder_awe_id().clone(),
        };
        self.history.push(change);
        Ok(self.history.last().expect("price change was just recorded"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Username};

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    #[test]
    fn only_the_founder_can_change_the_official_price() {
        let founder = identity("price-founder");
        let other = identity("price-other");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut control = OfficialPriceControl::new(100).unwrap();
        assert_eq!(
            control
                .set_price(
                    &mut authority,
                    &other.public.public_key,
                    200,
                    10,
                    5,
                    "growth"
                )
                .unwrap_err(),
            PriceControlError::Founder(FounderError::NotFounder)
        );
        assert_eq!(control.current_price(), 100);
        assert_eq!(control.history().len(), 0);
    }

    #[test]
    fn price_changes_have_effective_times_and_history() {
        let founder = identity("price-founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut control = OfficialPriceControl::new(100).unwrap();
        control
            .set_price(
                &mut authority,
                &founder.public.public_key,
                150,
                100,
                10,
                "demand grew",
            )
            .unwrap();
        control
            .set_price(
                &mut authority,
                &founder.public.public_key,
                120,
                200,
                150,
                "stabilizing",
            )
            .unwrap();
        assert_eq!(control.price_at(50), 100);
        assert_eq!(control.price_at(150), 150);
        assert_eq!(control.price_at(999), 120);
        let history = control.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].old_price_usd_cents, 100);
        assert_eq!(history[1].old_price_usd_cents, 150);
        assert_eq!(history[1].reason, "stabilizing");
        assert_eq!(history[1].changed_by, founder.public.awe_id);
        assert_eq!(history[1].sequence, 2);
    }

    #[test]
    fn invalid_price_reason_and_time_order_are_rejected() {
        let founder = identity("price-founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut control = OfficialPriceControl::new(100).unwrap();
        assert_eq!(
            control
                .set_price(&mut authority, &founder.public.public_key, 0, 10, 5, "x")
                .unwrap_err(),
            PriceControlError::InvalidPrice
        );
        assert_eq!(
            control
                .set_price(
                    &mut authority,
                    &founder.public.public_key,
                    200,
                    10,
                    5,
                    "   "
                )
                .unwrap_err(),
            PriceControlError::InvalidReason
        );
        control
            .set_price(
                &mut authority,
                &founder.public.public_key,
                200,
                100,
                5,
                "first",
            )
            .unwrap();
        assert_eq!(
            control
                .set_price(
                    &mut authority,
                    &founder.public.public_key,
                    300,
                    50,
                    6,
                    "backdated"
                )
                .unwrap_err(),
            PriceControlError::NonMonotonicEffectiveTime
        );
        assert_eq!(control.history().len(), 1);
    }

    #[test]
    fn the_official_price_is_clearly_labeled_as_internal() {
        assert!(PRICE_DISCLAIMER.contains("does not guarantee"));
    }
}
