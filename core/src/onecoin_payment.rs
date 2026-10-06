//! Native AWENET payment requests used by AWESTORE and services.
use crate::{identity::{AweId, Identity}, onecoin::{OnecoinLedger, OnecoinTransaction}};
use serde::{Deserialize, Serialize};

const PAYMENT_VERSION: u16 = 1;
const MAX_MEMO: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinPaymentRequest {
    pub version: u16,
    pub invoice_id: [u8; 32],
    pub seller: AweId,
    pub buyer: AweId,
    pub resource_id: [u8; 32],
    pub amount_atoms: u128,
    pub expires_at_unix: u64,
    pub memo: Option<String>,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

impl OnecoinPaymentRequest {
    pub fn new(
        seller: &Identity,
        invoice_id: [u8; 32],
        buyer: &AweId,
        resource_id: [u8; 32],
        amount_atoms: u128,
        expires_at_unix: u64,
        memo: Option<String>,
    ) -> Result<Self, String> {
        if amount_atoms == 0 { return Err("payment amount must be positive".into()); }
        if memo.as_ref().is_some_and(|m| m.len() > MAX_MEMO) { return Err("payment memo too long".into()); }
        let mut request = Self {
            version: PAYMENT_VERSION,
            invoice_id,
            seller: seller.public.awe_id.clone(),
            buyer: buyer.clone(),
            resource_id,
            amount_atoms,
            expires_at_unix,
            memo,
            signature: [0; 64],
        };
        request.signature = seller.sign(&request.signing_bytes());
        Ok(request)
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = [0; 64];
        serde_json::to_vec(&unsigned).expect("payment request serialization")
    }

    pub fn verify(&self, seller_public_key: &[u8; 32], now_unix: u64) -> bool {
        self.version == PAYMENT_VERSION
            && self.amount_atoms > 0
            && self.expires_at_unix >= now_unix
            && self.memo.as_ref().is_none_or(|m| m.len() <= MAX_MEMO)
            && self.seller.as_bytes() == AweId::from_public_key(seller_public_key).as_bytes()
            && Identity::verify(seller_public_key, &self.signing_bytes(), &self.signature)
    }

    pub fn payment_transaction(&self, buyer: &Identity, nonce: u64, memo: Option<String>) -> Result<OnecoinTransaction, String> {
        if buyer.public.awe_id != self.buyer { return Err("buyer identity does not match invoice".into()); }
        Ok(OnecoinTransaction::new(buyer, nonce, &self.seller, self.amount_atoms, memo.or_else(|| self.memo.clone())))
    }

    pub fn settle(&self, ledger: &mut OnecoinLedger, tx: &OnecoinTransaction, buyer_public_key: &[u8; 32], now_unix: u64) -> Result<[u8; 32], String> {
        if !self.verify(&ledger.members.get(&self.seller.to_hex()).copied().ok_or("seller is not a member")?, now_unix) {
            return Err("invalid or expired payment request".into());
        }
        if tx.recipient != *self.seller.as_bytes() || tx.sender != *self.buyer.as_bytes() || tx.amount_atoms != self.amount_atoms {
            return Err("transaction does not match payment request".into());
        }
        ledger.apply_transfer(tx, buyer_public_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    #[test]
    fn store_payment_request_binds_seller_buyer_resource_and_amount() {
        let seller = Identity::generate(Username::new("seller").unwrap());
        let buyer = Identity::generate(Username::new("buyer").unwrap());
        let request = OnecoinPaymentRequest::new(
            &seller, [9; 32], &buyer.public.awe_id, [8; 32], 123, 2_000, Some("app".into())
        ).unwrap();
        assert!(request.verify(&seller.public.public_key, 1_000));
        let tx = request.payment_transaction(&buyer, 0, None).unwrap();
        assert_eq!(tx.amount_atoms, 123);
        assert_eq!(tx.recipient, *seller.public.awe_id.as_bytes());
    }
}
