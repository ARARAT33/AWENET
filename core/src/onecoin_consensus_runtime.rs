//! Live ONECOIN consensus transport/runtime.
//!
//! Consensus messages travel over the authenticated AWE data plane. This module
//! deliberately keeps validator membership explicit: a node only participates
//! when its configured validator set contains its AWEID and public key.

use crate::identity::{AweId, Identity};
use crate::onecoin_consensus::{
    OnecoinBlock, OnecoinFinalizedState, QuorumCertificate, SignedBlockVote,
};
use crate::onecoin_store::PersistentOnecoinState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

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
    // Persisted before a vote is emitted to prevent equivocation across restarts.
    local_votes: BTreeMap<u64, [u8; 32]>,
    vote_lock_path: PathBuf,
}

impl OnecoinConsensusRuntime {
    pub fn open(
        path: impl AsRef<Path>,
        validators: BTreeMap<String, [u8; 32]>,
    ) -> Result<Self, String> {
        let state_path = path.as_ref().to_path_buf();
        let state = PersistentOnecoinState::open(&state_path)
            .map_err(|e| format!("failed to open ONECOIN consensus state: {e}"))?;
        let vote_lock_path = state_path.with_extension("vote-locks.json");
        let vote_lock_backup = state_path.with_extension("vote-locks.bak");
        // Recover a durable prior snapshot if the process stopped between
        // staging the old file and installing its replacement on Windows.
        if !vote_lock_path.exists() && vote_lock_backup.exists() {
            fs::rename(&vote_lock_backup, &vote_lock_path)
                .map_err(|e| format!("failed to recover ONECOIN vote-lock backup: {e}"))?;
        } else if vote_lock_path.exists() && vote_lock_backup.exists() {
            fs::remove_file(&vote_lock_backup)
                .map_err(|e| format!("failed to clean stale ONECOIN vote-lock backup: {e}"))?;
        }
        let mut local_votes: BTreeMap<u64, [u8; 32]> = if vote_lock_path.exists() {
            let bytes = fs::read(&vote_lock_path)
                .map_err(|e| format!("failed to read ONECOIN vote locks: {e}"))?;
            if bytes.len() > 1024 * 1024 {
                return Err("ONECOIN vote-lock file exceeds size limit".into());
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| "invalid ONECOIN vote-lock file; refusing to vote".to_string())?
        } else {
            BTreeMap::new()
        };
        if local_votes.len() > 4096 {
            return Err("ONECOIN vote-lock file has too many entries".into());
        }
        local_votes.retain(|height, _| *height > state.state.height);
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
            local_votes,
            vote_lock_path,
        })
    }

    fn persist_local_votes(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.local_votes)
            .map_err(|_| "failed to serialize ONECOIN vote locks".to_string())?;
        let temporary = self.vote_lock_path.with_extension("vote-locks.part");
        let mut file = fs::File::create(&temporary)
            .map_err(|e| format!("failed to create ONECOIN vote-lock file: {e}"))?;
        use std::io::Write;
        file.write_all(&bytes)
            .map_err(|e| format!("failed to write ONECOIN vote locks: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("failed to sync ONECOIN vote locks: {e}"))?;
        drop(file);

        // Windows may reject rename-over-existing-file. Stage the previous
        // lock file and restore it if installing the new durable snapshot fails.
        if !self.vote_lock_path.exists() {
            return fs::rename(&temporary, &self.vote_lock_path)
                .map_err(|e| format!("failed to persist ONECOIN vote locks: {e}"));
        }

        let backup = self.vote_lock_path.with_extension("vote-locks.bak");
        if backup.exists() {
            fs::remove_file(&backup)
                .map_err(|e| format!("failed to remove stale vote-lock backup: {e}"))?;
        }
        fs::rename(&self.vote_lock_path, &backup)
            .map_err(|e| format!("failed to stage previous ONECOIN vote locks: {e}"))?;
        if let Err(error) = fs::rename(&temporary, &self.vote_lock_path) {
            return match fs::rename(&backup, &self.vote_lock_path) {
                Ok(()) => Err(format!("failed to install ONECOIN vote locks: {error}")),
                Err(restore_error) => Err(format!(
                    "failed to install ONECOIN vote locks ({error}); rollback failed ({restore_error}); previous lock file remains at {}",
                    backup.display()
                )),
            };
        }
        let _ = fs::remove_file(&backup);
        Ok(())
    }

    pub fn local_is_validator(&self, identity: &Identity) -> bool {
        self.validators
            .get(&identity.public.awe_id.to_hex())
            .is_some_and(|key| key == &identity.public.public_key)
    }

    pub fn handle(
        &mut self,
        sender: [u8; 32],
        identity: &Identity,
        message: OnecoinConsensusMessage,
    ) -> Result<Vec<([u8; 32], OnecoinConsensusMessage)>, String> {
        let sender_id = AweId::from_public_key(&sender).to_hex();
        if self.validators.get(&sender_id) != Some(&sender) {
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
        if block.header.proposer != AweId::from_public_key(&sender) {
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
        // Reject invalid transactions before storing or voting on the proposal.
        self.state.state.preview_ledger(&block)?;
        let hash = block.hash();
        let is_local_validator = self.local_is_validator(identity);
        if is_local_validator
            && self
                .local_votes
                .get(&block.header.height)
                .is_some_and(|previous| previous != &hash)
        {
            return Err("refusing to vote for conflicting blocks at the same height".into());
        }
        if self.pending.len() >= MAX_PENDING_BLOCKS && !self.pending.contains_key(&hash) {
            return Err("too many pending consensus blocks".into());
        }
        self.pending.insert(hash, block.clone());

        if is_local_validator {
            let previous = self.local_votes.insert(block.header.height, hash);
            if let Err(error) = self.persist_local_votes() {
                if let Some(previous_hash) = previous {
                    self.local_votes.insert(block.header.height, previous_hash);
                } else {
                    self.local_votes.remove(&block.header.height);
                }
                return Err(error);
            }
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
        if vote.voter != AweId::from_public_key(&sender) {
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
            self.local_votes
                .retain(|height, _| *height > self.state.state.height);
            self.persist_local_votes()?;
            return Ok(self
                .validators
                .values()
                .copied()
                .map(|peer| (peer, OnecoinConsensusMessage::Certificate(cert.clone())))
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
        self.local_votes
            .retain(|height, _| *height > self.state.state.height);
        self.persist_local_votes()?;
        Ok(Vec::new())
    }

    pub fn persisted_state(&self) -> &OnecoinFinalizedState {
        &self.state.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::onecoin_consensus::OnecoinBlock;
    use std::fs;

    #[test]
    fn validator_refuses_conflicting_proposals_at_same_height() {
        let dir =
            std::env::temp_dir().join(format!("awenet-consensus-runtime-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let identity = Identity::generate(Username::new("validator").unwrap());
        let validators =
            BTreeMap::from([(identity.public.awe_id.to_hex(), identity.public.public_key)]);
        let state_path = dir.join("state.bin");
        let mut runtime = OnecoinConsensusRuntime::open(&state_path, validators.clone()).unwrap();
        let first = OnecoinBlock::new(&identity, 1, [0; 32], vec![], 1).unwrap();
        let conflicting = OnecoinBlock::new(&identity, 1, [0; 32], vec![], 2).unwrap();

        let first_result = runtime
            .handle(
                identity.public.public_key,
                &identity,
                OnecoinConsensusMessage::Proposal(first),
            )
            .unwrap();
        assert_eq!(first_result.len(), 1);
        drop(runtime);

        // The lock survives a process/runtime restart.
        let mut restarted = OnecoinConsensusRuntime::open(&state_path, validators).unwrap();
        assert!(restarted
            .handle(
                identity.public.public_key,
                &identity,
                OnecoinConsensusMessage::Proposal(conflicting),
            )
            .is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_vote_lock_replacement_recovers_backup() {
        let dir = std::env::temp_dir().join(format!(
            "awenet-consensus-vote-backup-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let identity = Identity::generate(Username::new("backup-validator").unwrap());
        let validators =
            BTreeMap::from([(identity.public.awe_id.to_hex(), identity.public.public_key)]);
        let state_path = dir.join("state.bin");
        let _runtime = OnecoinConsensusRuntime::open(&state_path, validators.clone()).unwrap();

        let vote_lock_path = state_path.with_extension("vote-locks.json");
        let backup_path = state_path.with_extension("vote-locks.bak");
        let recovered_hash = [7u8; 32];
        fs::write(
            &backup_path,
            serde_json::to_vec(&BTreeMap::from([(1u64, recovered_hash)])).unwrap(),
        )
        .unwrap();

        let runtime = OnecoinConsensusRuntime::open(&state_path, validators).unwrap();
        assert_eq!(runtime.local_votes.get(&1), Some(&recovered_hash));
        assert!(vote_lock_path.exists());
        assert!(!backup_path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn consensus_messages_reject_payloads_over_size_limit() {
        let bytes = vec![b' '; MAX_CONSENSUS_MESSAGE_BYTES + 1];
        assert!(OnecoinConsensusMessage::decode(&bytes).is_err());
    }
}
