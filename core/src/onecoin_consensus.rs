//! Deterministic ONECOIN transaction finalization.
//!
//! A local ledger is not sufficient for a distributed currency: two nodes can
//! otherwise accept conflicting spends. This module defines the protocol-level
//! block/finality primitive used to make transaction ordering deterministic.
//! Transport, peer discovery, and validator-set membership can remain separate.

use crate::identity::{AweId, Identity};
use crate::onecoin::{OnecoinLedger, OnecoinTransaction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CONSENSUS_VERSION: u16 = 1;
pub const MAX_BLOCK_TRANSACTIONS: usize = 4096;
pub const QUORUM_BPS: u64 = 6_667;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinBlockHeader {
    pub version: u16,
    pub height: u64,
    pub previous_hash: [u8; 32],
    pub transactions_hash: [u8; 32],
    pub proposer: AweId,
    pub timestamp_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinBlock {
    pub header: OnecoinBlockHeader,
    pub transactions: Vec<OnecoinTransaction>,
    #[serde(with = "crate::serde_bytes_64")]
    pub proposer_signature: [u8; 64],
}

impl OnecoinBlock {
    pub fn new(
        proposer: &Identity,
        height: u64,
        previous_hash: [u8; 32],
        mut transactions: Vec<OnecoinTransaction>,
        timestamp_unix: u64,
    ) -> Result<Self, String> {
        if transactions.len() > MAX_BLOCK_TRANSACTIONS {
            return Err("too many transactions in block".into());
        }
        // Canonical order prevents different proposers from producing different
        // state transitions from the same transaction set.
        transactions.sort_by_key(|tx| (tx.sender, tx.nonce, tx.id()));
        let transactions_hash = hash_transactions(&transactions);
        let mut block = Self {
            header: OnecoinBlockHeader {
                version: CONSENSUS_VERSION,
                height,
                previous_hash,
                transactions_hash,
                proposer: proposer.public.awe_id.clone(),
                timestamp_unix,
            },
            transactions,
            proposer_signature: [0; 64],
        };
        block.proposer_signature = proposer.sign(&block.signing_bytes());
        Ok(block)
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.proposer_signature = [0; 64];
        serde_json::to_vec(&unsigned).expect("ONECOIN block serialization")
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&serde_json::to_vec(self).expect("ONECOIN block serialization")).as_bytes()
    }

    pub fn verify_proposer(&self, public_key: &[u8; 32]) -> bool {
        self.header.version == CONSENSUS_VERSION
            && self.header.proposer == AweId::from_public_key(public_key)
            && Identity::verify(public_key, &self.signing_bytes(), &self.proposer_signature)
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if self.header.version != CONSENSUS_VERSION {
            return Err("unsupported ONECOIN consensus version".into());
        }
        if self.transactions.len() > MAX_BLOCK_TRANSACTIONS {
            return Err("block transaction limit exceeded".into());
        }
        if hash_transactions(&self.transactions) != self.header.transactions_hash {
            return Err("transaction set hash mismatch".into());
        }
        if self
            .transactions
            .windows(2)
            .any(|w| (w[0].sender, w[0].nonce, w[0].id()) > (w[1].sender, w[1].nonce, w[1].id()))
        {
            return Err("transactions are not in canonical order".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedBlockVote {
    pub block_hash: [u8; 32],
    pub height: u64,
    pub voter: AweId,
    pub approve: bool,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

impl SignedBlockVote {
    pub fn new(identity: &Identity, block: &OnecoinBlock, approve: bool) -> Self {
        let mut vote = Self {
            block_hash: block.hash(),
            height: block.header.height,
            voter: identity.public.awe_id.clone(),
            approve,
            signature: [0; 64],
        };
        vote.signature = identity.sign(&vote.signing_bytes());
        vote
    }

    fn signing_bytes(&self) -> Vec<u8> {
        let mut vote = self.clone();
        vote.signature = [0; 64];
        serde_json::to_vec(&vote).expect("ONECOIN vote serialization")
    }

    pub fn verify(&self, public_key: &[u8; 32]) -> bool {
        self.voter == AweId::from_public_key(public_key)
            && Identity::verify(public_key, &self.signing_bytes(), &self.signature)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuorumCertificate {
    pub block_hash: [u8; 32],
    pub height: u64,
    pub votes: Vec<SignedBlockVote>,
}

impl QuorumCertificate {
    pub fn verify(
        &self,
        validators: &BTreeMap<String, [u8; 32]>,
        expected_block_hash: [u8; 32],
        expected_height: u64,
    ) -> Result<(), String> {
        if self.block_hash != expected_block_hash || self.height != expected_height {
            return Err("quorum certificate does not match block".into());
        }
        let mut unique = BTreeMap::new();
        for vote in &self.votes {
            let key = vote.voter.to_hex();
            let Some(public_key) = validators.get(&key) else {
                continue;
            };
            if vote.block_hash != self.block_hash || vote.height != self.height {
                continue;
            }
            if vote.approve && vote.verify(public_key) {
                unique.insert(key, true);
            }
        }
        let approvals = unique.len() as u64;
        let total = validators.len() as u64;
        let required = total.saturating_mul(QUORUM_BPS).div_ceil(10_000);
        if total == 0 || approvals < required {
            return Err("validator quorum not reached".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinFinalizedState {
    pub ledger: OnecoinLedger,
    pub height: u64,
    pub tip_hash: [u8; 32],
}

impl Default for OnecoinFinalizedState {
    fn default() -> Self {
        Self {
            ledger: OnecoinLedger::default(),
            height: 0,
            tip_hash: [0; 32],
        }
    }
}

impl OnecoinFinalizedState {
    pub fn finalize(
        &mut self,
        block: &OnecoinBlock,
        certificate: &QuorumCertificate,
        validators: &BTreeMap<String, [u8; 32]>,
    ) -> Result<[u8; 32], String> {
        block.validate_shape()?;
        let proposer_key = validators
            .get(&block.header.proposer.to_hex())
            .ok_or("block proposer is not a validator")?;
        if !block.verify_proposer(proposer_key) {
            return Err("invalid block proposer signature".into());
        }
        if block.header.height != self.height.saturating_add(1) {
            return Err("invalid block height".into());
        }
        if block.header.previous_hash != self.tip_hash {
            return Err("invalid previous block hash".into());
        }
        certificate.verify(validators, block.hash(), block.header.height)?;

        // Execute against a clone first. A failed transaction must never leave
        // the live state half-applied.
        let mut next = self.ledger.clone();
        let mut seen = BTreeMap::new();
        for tx in &block.transactions {
            let txid = tx.id();
            if seen.insert(txid, true).is_some() {
                return Err("duplicate transaction in block".into());
            }
            let sender_key = next.ledger_sender_key(tx)?;
            next.apply_transfer(tx, &sender_key)?;
        }

        self.ledger = next;
        self.height = block.header.height;
        self.tip_hash = block.hash();
        Ok(self.tip_hash)
    }
}

fn hash_transactions(transactions: &[OnecoinTransaction]) -> [u8; 32] {
    let bytes = serde_json::to_vec(transactions).expect("ONECOIN transaction serialization");
    *blake3::hash(&bytes).as_bytes()
}

trait LedgerSenderKey {
    fn ledger_sender_key(&self, tx: &OnecoinTransaction) -> Result<[u8; 32], String>;
}

impl LedgerSenderKey for OnecoinLedger {
    fn ledger_sender_key(&self, tx: &OnecoinTransaction) -> Result<[u8; 32], String> {
        self.members
            .get(&AweId::from_public_key(&tx.sender).to_hex())
            .copied()
            .ok_or_else(|| "transaction sender is not a member".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::onecoin::INITIAL_GENESIS_ALLOCATION;

    fn id(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    #[test]
    fn finalized_block_requires_two_thirds_and_applies_atomically() {
        let a = id("a");
        let b = id("b");
        let c = id("c");
        let recipient = id("recipient");
        let mut state = OnecoinFinalizedState::default();
        state
            .ledger
            .initialize_genesis(&[
                a.public.awe_id.clone(),
                b.public.awe_id.clone(),
                c.public.awe_id.clone(),
                recipient.public.awe_id.clone(),
            ])
            .unwrap();
        let validators = BTreeMap::from([
            (a.public.awe_id.to_hex(), a.public.public_key),
            (b.public.awe_id.to_hex(), b.public.public_key),
            (c.public.awe_id.to_hex(), c.public.public_key),
        ]);
        let tx = OnecoinTransaction::new(&a, 0, &recipient.public.awe_id, 1, None);
        let block = OnecoinBlock::new(&a, 1, [0; 32], vec![tx], 10).unwrap();
        let cert = QuorumCertificate {
            block_hash: block.hash(),
            height: 1,
            votes: vec![
                SignedBlockVote::new(&a, &block, true),
                SignedBlockVote::new(&b, &block, true),
            ],
        };
        state.finalize(&block, &cert, &validators).unwrap();
        assert_eq!(
            state.ledger.balance_atoms(&a.public.awe_id),
            INITIAL_GENESIS_ALLOCATION - 1
        );
    }
}
