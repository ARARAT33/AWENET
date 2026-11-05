//! AWENET Store escrow: ONECOIN payments held until delivery.
//!
//! Store revenue is a buyer-to-seller payment, never new issuance. This
//! module bridges the Store and the escrow lifecycle: the buyer's
//! ONECOIN moves into the escrow operator's hold when the order is
//! funded, the seller receives the amount minus the transparent
//! marketplace fee once the buyer confirms delivery, the buyer is
//! refunded on expiry, and disputes are settled only by the founder
//! through the backend founder authority.

use crate::escrow::{EscrowError, EscrowState, Escrows};
use crate::founder::{FounderAction, FounderAuthority, FounderError};
use crate::identity::{AweId, Identity};
use crate::onecoin::{OnecoinLedger, OnecoinTransaction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_FEE_BPS: u16 = 500;
pub const MAX_ITEM_TITLE_CHARS: usize = 128;
pub const MAX_DISPUTE_REASON_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StoreEscrowError {
    #[error(transparent)]
    Escrow(#[from] EscrowError),
    #[error(transparent)]
    Founder(#[from] FounderError),
    #[error("ONECOIN ledger rejected the store payment: {0}")]
    Ledger(String),
    #[error("marketplace fee cannot exceed {MAX_FEE_BPS} bps")]
    InvalidFee,
    #[error("order {0} is unknown")]
    UnknownOrder(u64),
    #[error("only the buyer of order {0} may perform this operation")]
    NotBuyer(u64),
    #[error("caller is not the escrow operator of this marketplace")]
    NotOperator,
    #[error("order {0} is disputed and awaits founder resolution")]
    Disputed(u64),
    #[error("order {0} has no open dispute")]
    NotDisputed(u64),
    #[error("order {0} holds no funds")]
    NotHeld(u64),
    #[error("only the buyer or the seller may dispute order {0}")]
    NotParticipant(u64),
    #[error("order {0} is already disputed")]
    AlreadyDisputed(u64),
    #[error("buyer, seller and escrow operator must be three different accounts")]
    SelfDealing,
    #[error("item title exceeds {MAX_ITEM_TITLE_CHARS} characters")]
    TitleTooLong,
    #[error("dispute reason exceeds {MAX_DISPUTE_REASON_CHARS} characters")]
    ReasonTooLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    AwaitingPayment,
    Held,
    Disputed,
    Released,
    Refunded,
    Expired,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "awaiting_payment",
            Self::Held => "held",
            Self::Disputed => "disputed",
            Self::Released => "released",
            Self::Refunded => "refunded",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisputeRuling {
    RefundBuyer,
    ReleaseSeller,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dispute {
    pub opened_by: AweId,
    pub reason: String,
    pub opened_tick: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreOrder {
    pub escrow_id: u64,
    pub buyer: AweId,
    pub seller: AweId,
    pub amount_atoms: u128,
    pub fee_atoms: u128,
    pub title: String,
    pub created_tick: u64,
    pub expiry_tick: Option<u64>,
    pub funded: bool,
    pub dispute: Option<Dispute>,
}

#[derive(Debug)]
pub struct StoreEscrow {
    operator: AweId,
    fee_bps: u16,
    escrows: Escrows,
    orders: BTreeMap<u64, StoreOrder>,
}

impl StoreEscrow {
    pub fn new(operator: &Identity, max_orders: usize, fee_bps: u16) -> Result<Self, StoreEscrowError> {
        if fee_bps > MAX_FEE_BPS {
            return Err(StoreEscrowError::InvalidFee);
        }
        Ok(Self {
            operator: operator.public.awe_id.clone(),
            fee_bps,
            escrows: Escrows::new(max_orders)?,
            orders: BTreeMap::new(),
        })
    }

    pub fn operator(&self) -> &AweId {
        &self.operator
    }

    pub fn fee_bps(&self) -> u16 {
        self.fee_bps
    }

    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    pub fn order(&self, order_id: u64) -> Option<&StoreOrder> {
        self.orders.get(&order_id)
    }

    /// Opens a store order: the escrow terms are fixed up front (amount,
    /// marketplace fee, expiry) and no funds move until the buyer funds it.
    pub fn open_order(
        &mut self,
        buyer: &AweId,
        seller: &AweId,
        amount_atoms: u128,
        title: &str,
        now_tick: u64,
        expiry_tick: Option<u64>,
    ) -> Result<u64, StoreEscrowError> {
        if title.len() > MAX_ITEM_TITLE_CHARS {
            return Err(StoreEscrowError::TitleTooLong);
        }
        if *buyer == *seller || *buyer == self.operator || *seller == self.operator {
            return Err(StoreEscrowError::SelfDealing);
        }
        let fee_atoms = amount_atoms.saturating_mul(self.fee_bps as u128) / 10_000;
        let escrow_id = self.escrows.create(
            &buyer.to_hex(),
            &seller.to_hex(),
            amount_atoms,
            fee_atoms,
            now_tick,
            expiry_tick,
        )?;
        self.orders.insert(
            escrow_id,
            StoreOrder {
                escrow_id,
                buyer: buyer.clone(),
                seller: seller.clone(),
                amount_atoms,
                fee_atoms,
                title: title.to_string(),
                created_tick: now_tick,
                expiry_tick,
                funded: false,
                dispute: None,
            },
        );
        Ok(escrow_id)
    }

    fn transfer_from_operator(
        &self,
        ledger: &mut OnecoinLedger,
        operator: &Identity,
        recipient: &AweId,
        amount_atoms: u128,
        memo: String,
    ) -> Result<(), StoreEscrowError> {
        if operator.public.awe_id != self.operator {
            return Err(StoreEscrowError::NotOperator);
        }
        let nonce = ledger
            .nonces
            .get(&operator.public.awe_id.to_hex())
            .copied()
            .unwrap_or(0);
        let tx = OnecoinTransaction::new(operator, nonce, recipient, amount_atoms, Some(memo));
        ledger
            .apply_transfer(&tx, &operator.public.public_key)
            .map_err(StoreEscrowError::Ledger)?;
        Ok(())
    }

    /// The buyer pays the order amount into the escrow hold. If the escrow
    /// does not open (for example the order expired first), the payment is
    /// returned to the buyer immediately.
    pub fn fund(
        &mut self,
        ledger: &mut OnecoinLedger,
        buyer: &Identity,
        operator: &Identity,
        order_id: u64,
        now_tick: u64,
    ) -> Result<(), StoreEscrowError> {
        let order = self
            .orders
            .get(&order_id)
            .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
        if order.buyer != buyer.public.awe_id {
            return Err(StoreEscrowError::NotBuyer(order_id));
        }
        let amount_atoms = order.amount_atoms;
        let buyer_id = order.buyer.clone();
        let nonce = ledger
            .nonces
            .get(&buyer_id.to_hex())
            .copied()
            .unwrap_or(0);
        let tx = OnecoinTransaction::new(
            buyer,
            nonce,
            &self.operator,
            amount_atoms,
            Some(format!("store-order:{order_id}")),
        );
        ledger
            .apply_transfer(&tx, &buyer.public.public_key)
            .map_err(StoreEscrowError::Ledger)?;
        match self.escrows.fund(order_id, now_tick) {
            Ok(()) => {
                if let Some(order) = self.orders.get_mut(&order_id) {
                    order.funded = true;
                }
                Ok(())
            }
            Err(error) => {
                self.transfer_from_operator(
                    ledger,
                    operator,
                    &buyer_id,
                    amount_atoms,
                    format!("store-order-return:{order_id}"),
                )?;
                Err(StoreEscrowError::Escrow(error))
            }
        }
    }

    /// Confirms delivery: only the buyer may confirm, and only outside a
    /// dispute. The seller receives the amount minus the marketplace fee.
    pub fn confirm_delivery(
        &mut self,
        ledger: &mut OnecoinLedger,
        operator: &Identity,
        confirmer: &AweId,
        order_id: u64,
    ) -> Result<u128, StoreEscrowError> {
        let seller = {
            let order = self
                .orders
                .get(&order_id)
                .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
            if order.dispute.is_some() {
                return Err(StoreEscrowError::Disputed(order_id));
            }
            if order.buyer != *confirmer {
                return Err(StoreEscrowError::NotBuyer(order_id));
            }
            order.seller.clone()
        };
        let net_atoms = self.escrows.release(order_id)?;
        if let Some(order) = self.orders.get_mut(&order_id) {
            order.funded = false;
        }
        self.transfer_from_operator(
            ledger,
            operator,
            &seller,
            net_atoms,
            format!("store-order-release:{order_id}"),
        )?;
        Ok(net_atoms)
    }

    /// Refunds a held, undisputed order: the buyer receives the full
    /// amount back and the fee is voided.
    pub fn refund(
        &mut self,
        ledger: &mut OnecoinLedger,
        operator: &Identity,
        order_id: u64,
    ) -> Result<u128, StoreEscrowError> {
        let buyer = {
            let order = self
                .orders
                .get(&order_id)
                .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
            if order.dispute.is_some() {
                return Err(StoreEscrowError::Disputed(order_id));
            }
            order.buyer.clone()
        };
        let amount_atoms = self.escrows.refund(order_id)?;
        if let Some(order) = self.orders.get_mut(&order_id) {
            order.funded = false;
        }
        self.transfer_from_operator(
            ledger,
            operator,
            &buyer,
            amount_atoms,
            format!("store-order-refund:{order_id}"),
        )?;
        Ok(amount_atoms)
    }

    /// Opens a dispute on a held order; only the buyer or the seller may
    /// open it, and only the founder can settle it afterwards.
    pub fn dispute(
        &mut self,
        by: &AweId,
        order_id: u64,
        reason: &str,
        now_tick: u64,
    ) -> Result<(), StoreEscrowError> {
        if reason.len() > MAX_DISPUTE_REASON_CHARS {
            return Err(StoreEscrowError::ReasonTooLong);
        }
        let order = self
            .orders
            .get_mut(&order_id)
            .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
        if !order.funded {
            return Err(StoreEscrowError::NotHeld(order_id));
        }
        if order.buyer != *by && order.seller != *by {
            return Err(StoreEscrowError::NotParticipant(order_id));
        }
        if order.dispute.is_some() {
            return Err(StoreEscrowError::AlreadyDisputed(order_id));
        }
        order.dispute = Some(Dispute {
            opened_by: by.clone(),
            reason: reason.to_string(),
            opened_tick: now_tick,
        });
        Ok(())
    }

    /// Founder-only dispute settlement. The ruling pays out of the escrow
    /// hold: the buyer is refunded in full or the seller is paid the
    /// amount minus the fee. No other account can settle a store dispute.
    pub fn resolve_dispute(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        ledger: &mut OnecoinLedger,
        operator: &Identity,
        order_id: u64,
        ruling: DisputeRuling,
        at_unix: u64,
    ) -> Result<u128, StoreEscrowError> {
        {
            let order = self
                .orders
                .get(&order_id)
                .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
            if order.dispute.is_none() {
                return Err(StoreEscrowError::NotDisputed(order_id));
            }
            if !order.funded {
                return Err(StoreEscrowError::NotHeld(order_id));
            }
        }
        authority.authorize(
            actor_public_key,
            FounderAction::SettleDispute,
            at_unix,
            "settle store order dispute",
        )?;
        let (recipient, amount_atoms) = match ruling {
            DisputeRuling::RefundBuyer => {
                let buyer = self.orders.get(&order_id).expect("checked above").buyer.clone();
                (buyer, self.escrows.refund(order_id)?)
            }
            DisputeRuling::ReleaseSeller => {
                let seller = self.orders.get(&order_id).expect("checked above").seller.clone();
                (seller, self.escrows.release(order_id)?)
            }
        };
        if let Some(order) = self.orders.get_mut(&order_id) {
            order.funded = false;
            order.dispute = None;
        }
        self.transfer_from_operator(
            ledger,
            operator,
            &recipient,
            amount_atoms,
            format!("store-order-dispute:{order_id}"),
        )?;
        Ok(amount_atoms)
    }

    /// Expires held, undisputed orders whose expiry tick has passed and
    /// returns the held amounts to the buyers.
    pub fn expire(
        &mut self,
        ledger: &mut OnecoinLedger,
        operator: &Identity,
        now_tick: u64,
    ) -> Result<Vec<u64>, StoreEscrowError> {
        let due: Vec<u64> = self
            .orders
            .values()
            .filter(|order| {
                order.funded
                    && order.dispute.is_none()
                    && order.expiry_tick.is_some_and(|expiry| expiry <= now_tick)
            })
            .map(|order| order.escrow_id)
            .collect();
        let mut refunded = Vec::new();
        for order_id in due {
            let buyer = self
                .orders
                .get(&order_id)
                .expect("collected from the order map")
                .buyer
                .clone();
            let amount_atoms = self.escrows.refund(order_id)?;
            if let Some(order) = self.orders.get_mut(&order_id) {
                order.funded = false;
            }
            self.transfer_from_operator(
                ledger,
                operator,
                &buyer,
                amount_atoms,
                format!("store-order-expiry-refund:{order_id}"),
            )?;
            refunded.push(order_id);
        }
        Ok(refunded)
    }

    pub fn status(&self, order_id: u64, now_tick: u64) -> Result<OrderStatus, StoreEscrowError> {
        let order = self
            .orders
            .get(&order_id)
            .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
        let escrow = self
            .escrows
            .get(order_id)
            .ok_or(StoreEscrowError::UnknownOrder(order_id))?;
        if order.dispute.is_some() {
            return Ok(OrderStatus::Disputed);
        }
        Ok(match escrow.state {
            EscrowState::Created
                if order.expiry_tick.is_some_and(|expiry| expiry <= now_tick) =>
            {
                OrderStatus::Expired
            }
            EscrowState::Created => OrderStatus::AwaitingPayment,
            EscrowState::Funded => OrderStatus::Held,
            EscrowState::Released => OrderStatus::Released,
            EscrowState::Refunded => OrderStatus::Refunded,
            EscrowState::Expired => OrderStatus::Expired,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onecoin::INITIAL_GENESIS_ALLOCATION;

    fn identity(name: &str) -> Identity {
        use crate::identity::Username;
        Identity::generate(Username::new(name).unwrap())
    }
}
