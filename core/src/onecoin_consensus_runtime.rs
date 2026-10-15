//! Live ONECOIN consensus transport/runtime.
//!
//! Consensus messages travel over the authenticated AWE data plane. This module
//! deliberately keeps validator membership explicit: a node only participates
//! when its configured validator set contains its AWEID and public key.

use crate::identity::{AweId, Identity};
use crate::onecoin::OnecoinTransaction;
use crate::onecoin_consensus::{
    OnecoinBlock, OnecoinFinalizedState, QuorumCertificate, SignedBlockVote,
};
use crate::onecoin_store::PersistentOnecoinState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub const ONECOIN_CONSENSUS_STREAM: u32 = 200;
pub const MAX_CONSENSUS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PENDING_BLOCKS: usize = 256;
const MAX_VOTES_PER_BLOCK: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnecoinConsensusMessage {
    Proposal(OnecoinBlock),
    Vote(SignedBlockVote),
    Certificate(QuorumCertificate),
}

impl OnecoinConsensusMessage {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_CONSENSUS_MESSAGE_BYTES {
            return Err("ONECOIN consensus message too large".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_CONSENSUS_MESSAGE_BYTES {
            return Err("ONECOIN consensus message too large".into());
        }
        serde_json::from_slice(bytes).map_err(|e| format!("invalid ONECOIN consensus message: {e}"))
    }
}

#[derive(Clone, Debug)]
pub struct OnecoinConsensusRuntime {
    pub state: PersistentOnecoinState,
    pub validators: BTreeMap<String, [u8; 32]>,
    pending: HashMap<[u8; 32], OnecoinBlock>,
    votes: HashMap<[u8; 32], BTreeMap<String, SignedBlockVote>>,
}

impl OnecoinConsensusRuntime {
    pub fn open(
        path: impl AsRef<Path>,
        validators: BTreeMap<String, [u8; 32]>,
    ) -> Result<Self, String> {
        let state = PersistentOnecoinState::open(path)
            .map_err(|e| format!("failed to open ONECOIN consensus state: {e}"))?;
        if validators.is_empty() {
            return Err("ONECOIN validator set is empty".into());
        }
        for (id, key) in &validators {
            if AweId::from_public_key(key).to_hex() != *id {
                return Err("ONECOIN validator AWEID/public-key mismatch".into());
            }
        }
        Ok(Self {
            state,
            validators,
            pending: HashMap::new(),
            votes: HashMap::new(),
        })
    }

    pub fn local_is_validator(&self, identity: &Identity) -> bool {
        self.validators
            .get(&identity.public.awe_id.to_hex())
            .is_some_and(|key| key == &identity.public.public_key)
    }

    /// Propose a block from the current finalized tip and emit it to the configured validators.
    /// The local validator's signed vote is recorded immediately; state only advances once a quorum
    /// certificate is verified and persisted.
    pub fn propose(
        &mut self,
        identity: &Identity,
        transactions: Vec<OnecoinTransaction>,
        timestamp_unix: u64,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        if !self.local_is_validator(identity) {
            return Err("local identity is not an authorized ONECOIN validator".into());
        }
        if self.pending.len() >= MAX_PENDING_BLOCKS {
            return Err("too many pending consensus blocks".into());
        }
        let block = OnecoinBlock::new(
            identity,
            self.state.state.height.saturating_add(1),
            self.state.state.tip_hash,
            transactions,
            timestamp_unix,
        )?;
        block.validate_shape()?;
        self.state.state.validate_block_transactions(&block)?;
        let hash = block.hash();
        if self.pending.contains_key(&hash) {
            return Err("duplicate pending ONECOIN proposal".into());
        }
        self.record_local_vote(&block)?;
        self.pending.insert(hash, block.clone());

        // A proposer must count its own signed vote; otherwise a single-validator testnet can
        // never finalize and multi-validator networks waste one quorum slot.
        let local_vote = SignedBlockVote::new(identity, &block, true);
        let mut outbound = self.handle_vote(*identity.public.awe_id.as_bytes(), local_vote)?;
        for (validator_id, public_key) in &self.validators {
            if public_key != &identity.public.public_key {
                let peer_id = hex::decode(validator_id)
                    .map_err(|_| "invalid validator AWEID encoding".to_string())?;
                let peer_id: [u8; 32] = peer_id
                    .try_into()
                    .map_err(|_| "invalid validator AWEID length".to_string())?;
                outbound.push((peer_id, OnecoinConsensusMessage::Proposal(block.clone())));
            }
        }
        Ok(outbound)
    }

    pub fn handle(
        &mut self,
        sender: [u8; 32],
        identity: &Identity,
        message: OnecoinConsensusMessage,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        // The transport authenticates peers by AWEID, not by raw public key.
        let sender_id = hex::encode(sender);
        if !self.validators.contains_key(&sender_id) {
            return Err("consensus sender is not a validator".into());
        }
        match message {
            OnecoinConsensusMessage::Proposal(block) => {
                self.handle_proposal(sender, identity, block)
            }
            OnecoinConsensusMessage::Vote(vote) => self.handle_vote(sender, vote),
            OnecoinConsensusMessage::Certificate(cert) => self.handle_certificate(cert),
        }
    }

    fn handle_proposal(
        &mut self,
        sender: [u8; 32],
        identity: &Identity,
        block: OnecoinBlock,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        block.validate_shape()?;
        if block.header.proposer.as_bytes() != &sender {
            return Err("proposal sender does not match block proposer".into());
        }
        let proposer_key = self
            .validators
            .get(&block.header.proposer.to_hex())
            .ok_or("proposal proposer is not a validator")?;
        if !block.verify_proposer(proposer_key) {
            return Err("invalid proposal signature".into());
        }
        if block.header.height != self.state.state.height + 1
            || block.header.previous_hash != self.state.state.tip_hash
        {
            return Err("proposal does not extend finalized state".into());
        }
        // Never vote for a block whose transactions would fail deterministic execution.
        self.state.state.validate_block_transactions(&block)?;
        if self.pending.len() >= MAX_PENDING_BLOCKS && !self.pending.contains_key(&block.hash()) {
            return Err("too many pending consensus blocks".into());
        }
        let hash = block.hash();
        if self.local_is_validator(identity) {
            self.record_local_vote(&block)?;
        }
        self.pending.insert(hash, block.clone());

        if self.local_is_validator(identity) {
            let vote = SignedBlockVote::new(identity, &block, true);
            Ok(vec![(sender, OnecoinConsensusMessage::Vote(vote))])
        } else {
            Ok(Vec::new())
        }
    }

    fn handle_vote(
        &mut self,
        sender: [u8; 32],
        vote: SignedBlockVote,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        if vote.voter.as_bytes() != &sender {
            return Err("vote sender does not match voter".into());
        }
        let key = self
            .validators
            .get(&vote.voter.to_hex())
            .ok_or("vote voter is not a validator")?;
        if !vote.verify(key) {
            return Err("invalid vote signature".into());
        }
        let block = self
            .pending
            .get(&vote.block_hash)
            .ok_or("vote references unknown block")?;
        if block.header.height != vote.height {
            return Err("vote height mismatch".into());
        }
        let entry = self.votes.entry(vote.block_hash).or_default();
        if entry.len() >= MAX_VOTES_PER_BLOCK && !entry.contains_key(&vote.voter.to_hex()) {
            return Err("too many votes for pending block".into());
        }
        let vote_hash = vote.block_hash;
        let vote_id = vote.voter.to_hex();
        entry.insert(vote_id, vote.clone());

        let cert = QuorumCertificate {
            block_hash: vote_hash,
            height: block.header.height,
            votes: entry.values().cloned().collect(),
        };
        if cert
            .verify(&self.validators, block.hash(), block.header.height)
            .is_ok()
        {
            self.state.finalize(block, &cert, &self.validators)?;
            self.pending.remove(&vote.block_hash);
            self.votes.remove(&vote.block_hash);
            return Ok(self
                .validators
                .values()
                .copied()
                .map(|public_key| {
                    let peer_id = AweId::from_public_key(&public_key);
                    (
                        *peer_id.as_bytes(),
                        OnecoinConsensusMessage::Certificate(cert.clone()),
                    )
                })
                .collect());
        }
        Ok(Vec::new())
    }

    fn handle_certificate(
        &mut self,
        cert: QuorumCertificate,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        let block = self
            .pending
            .get(&cert.block_hash)
            .ok_or("certificate references unknown block")?
            .clone();
        self.state.finalize(&block, &cert, &self.validators)?;
        self.pending.remove(&cert.block_hash);
        self.votes.remove(&cert.block_hash);
        Ok(Vec::new())
    }

    fn record_local_vote(&mut self, block: &OnecoinBlock) -> Result<(), String> {
        let height = block.header.height;
        let hash = block.hash();
        if let Some(previous) = self.state.state.local_votes.get(&height) {
            if previous != &hash {
                return Err(
                    "local validator already voted for a different block at this height".into(),
                );
            }
            return Ok(());
        }
        self.state.state.local_votes.insert(height, hash);
        if let Err(error) = self.state.save() {
            self.state.state.local_votes.remove(&height);
            return Err(format!(
                "failed to persist local validator vote lock: {error}"
            ));
        }
        Ok(())
    }

    pub fn persisted_state(&self) -> &OnecoinFinalizedState {
        &self.state.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use std::fs;

    #[test]
    fn single_validator_proposal_finalizes_and_persists() {
        let dir =
            std::env::temp_dir().join(format!("awenet-consensus-propose-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let identity = Identity::generate(Username::new("validator").unwrap());
        let validators =
            BTreeMap::from([(identity.public.awe_id.to_hex(), identity.public.public_key)]);
        let mut runtime = OnecoinConsensusRuntime::open(&path, validators.clone()).unwrap();
        runtime
            .state
            .state
            .ledger
            .initialize_genesis(&[identity.public.awe_id.clone()])
            .unwrap();
        runtime.state.save().unwrap();

        let outbound = runtime.propose(&identity, Vec::new(), 100).unwrap();
        assert_eq!(runtime.persisted_state().height, 1);
        assert!(outbound
            .iter()
            .any(|(_, message)| matches!(message, OnecoinConsensusMessage::Certificate(_))));

        let reopened = OnecoinConsensusRuntime::open(&path, validators).unwrap();
        assert_eq!(reopened.persisted_state().height, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validator_rejects_unspendable_transaction_before_voting() {
        use crate::onecoin::INITIAL_GENESIS_ALLOCATION;

        let dir = std::env::temp_dir().join(format!(
            "awenet-consensus-invalid-tx-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let proposer = Identity::generate(Username::new("proposer").unwrap());
        let validator = Identity::generate(Username::new("validator2").unwrap());
        let recipient = Identity::generate(Username::new("recipient").unwrap());
        let validators = BTreeMap::from([
            (proposer.public.awe_id.to_hex(), proposer.public.public_key),
            (
                validator.public.awe_id.to_hex(),
                validator.public.public_key,
            ),
        ]);
        let mut runtime =
            OnecoinConsensusRuntime::open(dir.join("state.json"), validators).unwrap();
        runtime
            .state
            .state
            .ledger
            .initialize_genesis(&[
                proposer.public.awe_id.clone(),
                validator.public.awe_id.clone(),
                recipient.public.awe_id.clone(),
            ])
            .unwrap();
        runtime.state.save().unwrap();
        let tx = OnecoinTransaction::new(
            &proposer,
            0,
            &recipient.public.awe_id,
            INITIAL_GENESIS_ALLOCATION + 1,
            None,
        );
        let block = OnecoinBlock::new(
            &proposer,
            1,
            runtime.persisted_state().tip_hash,
            vec![tx],
            101,
        )
        .unwrap();
        let result = runtime.handle(
            *proposer.public.awe_id.as_bytes(),
            &validator,
            OnecoinConsensusMessage::Proposal(block),
        );
        assert!(result.is_err());
        assert_eq!(runtime.persisted_state().height, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validator_does_not_double_vote_even_after_restart() {
        let dir = std::env::temp_dir().join(format!(
            "awenet-consensus-double-vote-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let proposer_a = Identity::generate(Username::new("proposer-a").unwrap());
        let proposer_b = Identity::generate(Username::new("proposer-b").unwrap());
        let local = Identity::generate(Username::new("local-validator").unwrap());
        let validators = BTreeMap::from([
            (
                proposer_a.public.awe_id.to_hex(),
                proposer_a.public.public_key,
            ),
            (
                proposer_b.public.awe_id.to_hex(),
                proposer_b.public.public_key,
            ),
            (local.public.awe_id.to_hex(), local.public.public_key),
        ]);
        let mut runtime = OnecoinConsensusRuntime::open(&path, validators.clone()).unwrap();
        runtime
            .state
            .state
            .ledger
            .initialize_genesis(&[
                proposer_a.public.awe_id.clone(),
                proposer_b.public.awe_id.clone(),
                local.public.awe_id.clone(),
            ])
            .unwrap();
        runtime.state.save().unwrap();
        let first = OnecoinBlock::new(&proposer_a, 1, [0; 32], Vec::new(), 100).unwrap();
        let conflicting = OnecoinBlock::new(&proposer_b, 1, [0; 32], Vec::new(), 101).unwrap();
        runtime
            .handle(
                *proposer_a.public.awe_id.as_bytes(),
                &local,
                OnecoinConsensusMessage::Proposal(first),
            )
            .unwrap();
        assert!(runtime
            .handle(
                *proposer_b.public.awe_id.as_bytes(),
                &local,
                OnecoinConsensusMessage::Proposal(conflicting.clone()),
            )
            .is_err());

        let mut reopened = OnecoinConsensusRuntime::open(&path, validators).unwrap();
        assert!(reopened
            .handle(
                *proposer_b.public.awe_id.as_bytes(),
                &local,
                OnecoinConsensusMessage::Proposal(conflicting),
            )
            .is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_validator_cannot_propose() {
        let dir =
            std::env::temp_dir().join(format!("awenet-consensus-deny-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let validator = Identity::generate(Username::new("validator").unwrap());
        let outsider = Identity::generate(Username::new("outsider").unwrap());
        let validators = BTreeMap::from([(
            validator.public.awe_id.to_hex(),
            validator.public.public_key,
        )]);
        let mut runtime =
            OnecoinConsensusRuntime::open(dir.join("state.json"), validators).unwrap();
        assert!(runtime.propose(&outsider, Vec::new(), 100).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
