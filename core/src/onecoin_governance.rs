//! ONECOIN collective price-floor governance primitives.
use crate::identity::{AweId, Identity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const GOVERNANCE_VERSION: u16 = 1;
pub const APPROVAL_BPS: u64 = 6_666;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceFloorProposal {
    pub version: u16,
    pub proposal_id: [u8; 32],
    pub current_floor_usd_cents: u64,
    pub proposed_floor_usd_cents: u64,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedFloorVote {
    pub proposal_id: [u8; 32],
    pub voter: AweId,
    pub approve: bool,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

impl SignedFloorVote {
    pub fn new(identity: &Identity, proposal_id: [u8; 32], approve: bool) -> Self {
        let mut vote = Self {
            proposal_id,
            voter: identity.public.awe_id.clone(),
            approve,
            signature: [0; 64],
        };
        vote.signature = identity.sign(&vote.signing_bytes());
        vote
    }
    fn signing_bytes(&self) -> Vec<u8> {
        let mut v = self.clone();
        v.signature = [0; 64];
        serde_json::to_vec(&v).expect("vote serialization")
    }
    pub fn verify(&self, public_key: &[u8; 32]) -> bool {
        self.voter.as_bytes() == AweId::from_public_key(public_key).as_bytes()
            && Identity::verify(public_key, &self.signing_bytes(), &self.signature)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PriceFloorGovernance {
    votes: BTreeMap<String, bool>,
}
impl PriceFloorGovernance {
    pub fn approve_if_quorum(
        &mut self,
        proposal: &PriceFloorProposal,
        votes: &[SignedFloorVote],
        members: &BTreeMap<String, [u8; 32]>,
        now_unix: u64,
    ) -> Result<bool, String> {
        if proposal.version != GOVERNANCE_VERSION
            || proposal.proposed_floor_usd_cents >= proposal.current_floor_usd_cents
        {
            return Err("proposal must lower the current floor".into());
        }
        if now_unix > proposal.expires_at_unix {
            return Err("proposal expired".into());
        }
        self.votes.clear();
        for vote in votes {
            let key = vote.voter.to_hex();
            let Some(pk) = members.get(&key) else {
                continue;
            };
            if vote.proposal_id != proposal.proposal_id || !vote.verify(pk) {
                continue;
            }
            self.votes.entry(key).or_insert(vote.approve);
        }
        let total = members.len() as u64;
        let approvals = self.votes.values().filter(|v| **v).count() as u64;
        let required = total.saturating_mul(APPROVAL_BPS).div_ceil(10_000);
        Ok(total > 0 && approvals >= required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    #[test]
    fn floor_reduction_requires_two_thirds_supermajority() {
        let a = Identity::generate(Username::new("a").unwrap());
        let b = Identity::generate(Username::new("b").unwrap());
        let c = Identity::generate(Username::new("c").unwrap());
        let members = BTreeMap::from([
            (a.public.awe_id.to_hex(), a.public.public_key),
            (b.public.awe_id.to_hex(), b.public.public_key),
            (c.public.awe_id.to_hex(), c.public.public_key),
        ]);
        let proposal = PriceFloorProposal {
            version: GOVERNANCE_VERSION,
            proposal_id: [1; 32],
            current_floor_usd_cents: 2_100,
            proposed_floor_usd_cents: 2_000,
            created_at_unix: 1,
            expires_at_unix: 100,
        };
        let mut g = PriceFloorGovernance::default();
        assert!(!g
            .approve_if_quorum(
                &proposal,
                &[SignedFloorVote::new(&a, [1; 32], true)],
                &members,
                2
            )
            .unwrap());
        assert!(g
            .approve_if_quorum(
                &proposal,
                &[
                    SignedFloorVote::new(&a, [1; 32], true),
                    SignedFloorVote::new(&b, [1; 32], true)
                ],
                &members,
                2
            )
            .unwrap());
    }
}
