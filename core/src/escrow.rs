//! Escrow lifecycle with fees, release, refund and expiry.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. An escrow
//! holds an amount between a buyer and a seller: it is funded once, then
//! released (seller receives amount minus fee) or refunded (buyer receives
//! the amount back), and may expire while unfunded or held.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_ESCROWS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum EscrowError {
    #[error("escrow capacity must be between 1 and {MAX_ESCROWS}")]
    InvalidCapacity,
    #[error("escrow registry is full (max {MAX_ESCROWS})")]
    RegistryFull,
    #[error("escrow amount must be greater than zero")]
    ZeroAmount,
    #[error("buyer and seller must differ")]
    SameParty,
    #[error("buyer and seller must not be empty")]
    EmptyParty,
    #[error("fee {fee} exceeds the escrow amount {amount}")]
    FeeExceedsAmount { fee: u64, amount: u64 },
    #[error("unknown escrow {0}")]
    UnknownEscrow(u64),
    #[error("escrow {0} is not funded")]
    NotFunded(u64),
    #[error("escrow {0} is in state {1:?}")]
    InvalidState(u64, EscrowState),
    #[error("escrow expiry must be after creation")]
    InvalidExpiry,
    #[error("escrow id space is exhausted")]
    IdExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EscrowState {
    Created,
    Funded,
    Released,
    Refunded,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Escrow {
    pub id: u64,
    pub buyer: String,
    pub seller: String,
    pub amount: u64,
    pub fee: u64,
    pub state: EscrowState,
    pub created_tick: u64,
    pub expiry_tick: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Escrows {
    max_escrows: usize,
    next_id: u64,
    escrows: BTreeMap<u64, Escrow>,
}

impl Escrows {
    pub fn new(max_escrows: usize) -> Result<Self, EscrowError> {
        if max_escrows == 0 || max_escrows > MAX_ESCROWS {
            return Err(EscrowError::InvalidCapacity);
        }
        Ok(Self {
            max_escrows,
            next_id: 1,
            escrows: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.escrows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.escrows.len() == 0
    }

    pub fn get(&self, id: u64) -> Option<&Escrow> {
        self.escrows.get(&id)
    }

    /// Creates an unfunded escrow between two distinct parties.
    pub fn create(
        &mut self,
        buyer: &str,
        seller: &str,
        amount: u64,
        fee: u64,
        created_tick: u64,
        expiry_tick: Option<u64>,
    ) -> Result<u64, EscrowError> {
        if amount == 0 {
            return Err(EscrowError::ZeroAmount);
        }
        if buyer.is_empty() || seller.is_empty() {
            return Err(EscrowError::EmptyParty);
        }
        if buyer == seller {
            return Err(EscrowError::SameParty);
        }
        if expiry_tick.is_some_and(|expiry| expiry <= created_tick) {
            return Err(EscrowError::InvalidExpiry);
        }
        if fee > amount {
            return Err(EscrowError::FeeExceedsAmount { fee, amount });
        }
        if self.escrows.len() >= self.max_escrows {
            return Err(EscrowError::RegistryFull);
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(EscrowError::IdExhausted)?;
        self.escrows.insert(
            id,
            Escrow {
                id,
                buyer: buyer.to_string(),
                seller: seller.to_string(),
                amount,
                fee,
                state: EscrowState::Created,
                created_tick,
                expiry_tick,
            },
        );
        Ok(id)
    }

    /// Locks the amount into the escrow. Expired escrows cannot be funded.
    pub fn fund(&mut self, id: u64, now: u64) -> Result<(), EscrowError> {
        let escrow = self
            .escrows
            .get_mut(&id)
            .ok_or(EscrowError::UnknownEscrow(id))?;
        if escrow.state != EscrowState::Created {
            return Err(EscrowError::InvalidState(id, escrow.state));
        }
        if matches!(escrow.expiry_tick, Some(expiry) if expiry <= now) {
            escrow.state = EscrowState::Expired;
            return Err(EscrowError::InvalidState(id, EscrowState::Expired));
        }
        escrow.state = EscrowState::Funded;
        Ok(())
    }

    /// Releases the funds to the seller: the seller receives the amount
    /// minus the fee, and the fee is kept as settlement.
    pub fn release(&mut self, id: u64) -> Result<u64, EscrowError> {
        let escrow = self
            .escrows
            .get_mut(&id)
            .ok_or(EscrowError::UnknownEscrow(id))?;
        if escrow.state != EscrowState::Funded {
            return Err(EscrowError::NotFunded(id));
        }
        escrow.state = EscrowState::Released;
        Ok(escrow.amount - escrow.fee)
    }

    /// Refunds the full amount to the buyer; the fee is voided.
    pub fn refund(&mut self, id: u64) -> Result<u64, EscrowError> {
        let escrow = self
            .escrows
            .get_mut(&id)
            .ok_or(EscrowError::UnknownEscrow(id))?;
        if escrow.state != EscrowState::Funded {
            return Err(EscrowError::NotFunded(id));
        }
        escrow.state = EscrowState::Refunded;
        Ok(escrow.amount)
    }

    /// Expires unfunded and held escrows whose expiry tick has passed and
    /// returns their ids. Held funds are returned to the buyer.
    pub fn expire(&mut self, now: u64) -> Vec<u64> {
        let mut expired = Vec::new();
        for escrow in self.escrows.values_mut() {
            let open = matches!(escrow.state, EscrowState::Created | EscrowState::Funded);
            if open && matches!(escrow.expiry_tick, Some(expiry) if expiry <= now) {
                escrow.state = EscrowState::Expired;
                expired.push(escrow.id);
            }
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_capacity_and_terms() {
        assert_eq!(Escrows::new(0).unwrap_err(), EscrowError::InvalidCapacity);
        assert_eq!(
            Escrows::new(MAX_ESCROWS + 1).unwrap_err(),
            EscrowError::InvalidCapacity
        );
        let mut escrows = Escrows::new(4).unwrap();
        assert_eq!(
            escrows.create("a", "b", 0, 0, 0, None).unwrap_err(),
            EscrowError::ZeroAmount
        );
        assert_eq!(
            escrows.create("a", "a", 10, 0, 0, None).unwrap_err(),
            EscrowError::SameParty
        );
        assert_eq!(
            escrows.create("a", "b", 10, 11, 0, None).unwrap_err(),
            EscrowError::FeeExceedsAmount {
                fee: 11,
                amount: 10
            }
        );
        assert_eq!(
            escrows.create("", "b", 10, 0, 0, None).unwrap_err(),
            EscrowError::EmptyParty
        );
        assert_eq!(
            escrows.create("a", "b", 10, 0, 10, Some(10)).unwrap_err(),
            EscrowError::InvalidExpiry
        );
    }

    #[test]
    fn registry_full_is_rejected() {
        let mut escrows = Escrows::new(1).unwrap();
        escrows.create("a", "b", 10, 1, 0, None).unwrap();
        assert_eq!(
            escrows.create("c", "d", 10, 1, 0, None).unwrap_err(),
            EscrowError::RegistryFull
        );
        assert_eq!(escrows.len(), 1);
        assert!(!escrows.is_empty());
    }

    #[test]
    fn fund_release_pays_seller_net_of_fee() {
        let mut escrows = Escrows::new(4).unwrap();
        let id = escrows.create("alice", "bob", 100, 5, 0, None).unwrap();
        assert_eq!(escrows.release(id).unwrap_err(), EscrowError::NotFunded(id));
        escrows.fund(id, 1).unwrap();
        assert_eq!(escrows.get(id).unwrap().state, EscrowState::Funded);
        assert_eq!(escrows.release(id).unwrap(), 95);
        assert_eq!(escrows.get(id).unwrap().state, EscrowState::Released);
        assert_eq!(escrows.release(id).unwrap_err(), EscrowError::NotFunded(id));
    }

    #[test]
    fn fund_refund_returns_the_full_amount() {
        let mut escrows = Escrows::new(4).unwrap();
        let id = escrows.create("alice", "bob", 100, 5, 0, None).unwrap();
        escrows.fund(id, 1).unwrap();
        assert_eq!(escrows.refund(id).unwrap(), 100);
        assert_eq!(escrows.get(id).unwrap().state, EscrowState::Refunded);
        assert_eq!(
            escrows.refund(99).unwrap_err(),
            EscrowError::UnknownEscrow(99)
        );
    }

    #[test]
    fn expiry_gates_funding_and_sweeps_holds() {
        let mut escrows = Escrows::new(8).unwrap();
        let unfunded = escrows.create("alice", "bob", 100, 5, 0, Some(10)).unwrap();
        let held = escrows.create("carol", "dave", 50, 0, 0, Some(10)).unwrap();
        let fresh = escrows
            .create("erin", "frank", 50, 0, 0, Some(100))
            .unwrap();
        assert_eq!(
            escrows.fund(unfunded, 10).unwrap_err(),
            EscrowError::InvalidState(unfunded, EscrowState::Expired)
        );
        escrows.fund(held, 5).unwrap();
        assert_eq!(escrows.expire(10), vec![held]);
        assert_eq!(escrows.get(held).unwrap().state, EscrowState::Expired);
        assert_eq!(escrows.get(fresh).unwrap().state, EscrowState::Created);
        assert!(escrows.expire(10).is_empty());
    }

    #[test]
    fn escrows_serialize_and_restore_exactly() {
        let mut escrows = Escrows::new(4).unwrap();
        let id = escrows.create("alice", "bob", 100, 5, 10, Some(99)).unwrap();
        escrows.fund(id, 20).unwrap();
        let bytes = serde_json::to_vec(&escrows).unwrap();
        let mut restored: Escrows = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.len(), 1);
        let escrow = restored.get(id).unwrap();
        assert_eq!(escrow.state, EscrowState::Funded);
        assert_eq!(escrow.amount, 100);
        assert_eq!(escrow.fee, 5);
        assert_eq!(escrow.expiry_tick, Some(99));
        // The restored registry continues from the persisted id space.
        let next = restored.create("carol", "dave", 7, 0, 30, None).unwrap();
        assert_eq!(next, id + 1);
    }
}
