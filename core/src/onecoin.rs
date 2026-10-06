//! ONECOIN monetary protocol primitives for AWENET.
//!
//! ONECOIN is earned by verified network contribution and can be transferred
//! without exposing human-readable identities in the transaction itself.
//! This module is deterministic and integer-only: no floating point is used
//! for balances, issuance, or price bands.

use crate::awenet::ContributionReceipt;
use crate::identity::{AweId, Identity};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ONECOIN_PROTOCOL: &str = "ONECOIN/1";
pub const ATOMS_PER_COIN: u128 = 1_000_000_000_000_000_000;
pub const INITIAL_GENESIS_ALLOCATION: u128 = 10 * ATOMS_PER_COIN;
pub const JOIN_DISTRIBUTION: u128 = ATOMS_PER_COIN;
pub const MIN_JOIN_SHARE: u128 = 1;
pub const INITIAL_PRICE_USD_CENTS: u64 = 100;
pub const PRICE_BAND_USD_CENTS: u64 = 10_000;
pub const PRICE_FLOOR_STEP_USD_CENTS: u64 = 2_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinAmount(pub u128);

impl OnecoinAmount {
    pub fn zero() -> Self { Self(0) }
    pub fn coins(coins: u128) -> Self { Self(coins.saturating_mul(ATOMS_PER_COIN)) }
    pub fn as_atoms(&self) -> u128 { self.0 }
    pub fn whole_coins(&self) -> u128 { self.0 / ATOMS_PER_COIN }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinTransaction {
    pub version: u16,
    pub nonce: u64,
    pub sender: [u8; 32],
    pub recipient: [u8; 32],
    pub amount_atoms: u128,
    pub memo: Option<String>,
    pub signature: [u8; 64],
}

impl OnecoinTransaction {
    pub fn new(
        identity: &Identity,
        nonce: u64,
        recipient: &AweId,
        amount_atoms: u128,
        memo: Option<String>,
    ) -> Self {
        let mut tx = Self {
            version: 1,
            nonce,
            sender: *identity.public.awe_id.as_bytes(),
            recipient: *recipient.as_bytes(),
            amount_atoms,
            memo,
            signature: [0u8; 64],
        };
        tx.signature = identity.sign(&tx.signing_bytes());
        tx
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = [0u8; 64];
        serde_json::to_vec(&unsigned).expect("ONECOIN transaction serialization")
    }

    pub fn id(&self) -> [u8; 32] {
        *blake3::hash(&serde_json::to_vec(self).expect("ONECOIN transaction serialization")).as_bytes()
    }

    pub fn verify(&self, sender_public_key: &[u8; 32]) -> bool {
        if self.version != 1 || self.amount_atoms == 0 { return false; }
        let Ok(key) = VerifyingKey::from_bytes(sender_public_key) else { return false; };
        let signature = Signature::from_bytes(&self.signature);
        key.verify(&self.signing_bytes(), &signature).is_ok()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionReward {
    pub node: AweId,
    pub receipt_hash: [u8; 32],
    pub reward_atoms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionRewardPolicy {
    pub atoms_per_score: u128,
    pub max_reward_atoms: u128,
}

impl Default for ContributionRewardPolicy {
    fn default() -> Self {
        Self { atoms_per_score: 1, max_reward_atoms: 100 * ATOMS_PER_COIN }
    }
}

impl ContributionRewardPolicy {
    pub fn reward_for(&self, receipt: &ContributionReceipt) -> Result<u128, String> {
        if receipt.period_end_unix <= receipt.period_start_unix {
            return Err("invalid contribution period".into());
        }
        let score = receipt.score();
        if score == 0 {
            return Err("empty contribution cannot earn ONECOIN".into());
        }
        Ok(score.saturating_mul(self.atoms_per_score).min(self.max_reward_atoms))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedContributionReceipt {
    pub receipt: ContributionReceipt,
    pub verifier_public_key: [u8; 32],
    pub signature: [u8; 64],
}

impl SignedContributionReceipt {
    pub fn new(verifier: &Identity, receipt: ContributionReceipt) -> Self {
        let mut signed = Self {
            receipt,
            verifier_public_key: verifier.public.public_key,
            signature: [0u8; 64],
        };
        signed.signature = verifier.sign(&signed.signing_bytes());
        signed
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = [0u8; 64];
        serde_json::to_vec(&unsigned).expect("signed contribution receipt serialization")
    }

    pub fn verify(&self) -> bool {
        Identity::verify(&self.verifier_public_key, &self.signing_bytes(), &self.signature)
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&serde_json::to_vec(self).expect("signed contribution receipt serialization")).as_bytes()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinDistribution {
    pub new_member: AweId,
    pub eligible_members: u64,
    pub per_member_atoms: u128,
    pub distributed_atoms: u128,
    pub remainder_atoms: u128,
    pub active: bool,
}

impl JoinDistribution {
    pub fn calculate(new_member: AweId, member_count_after_join: u64) -> Self {
        let n = member_count_after_join.max(1) as u128;
        let per_member_atoms = JOIN_DISTRIBUTION / n;
        let distributed_atoms = per_member_atoms.saturating_mul(n);
        let remainder_atoms = JOIN_DISTRIBUTION.saturating_sub(distributed_atoms);
        Self {
            new_member,
            eligible_members: member_count_after_join,
            per_member_atoms,
            distributed_atoms,
            remainder_atoms,
            active: per_member_atoms >= MIN_JOIN_SHARE,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinPricePolicy {
    pub floor_usd_cents: u64,
    pub highest_band_reached: u64,
}

impl Default for OnecoinPricePolicy {
    fn default() -> Self {
        Self {
            floor_usd_cents: INITIAL_PRICE_USD_CENTS,
            highest_band_reached: 0,
        }
    }
}

impl OnecoinPricePolicy {
    /// Raises the protocol floor by $20 whenever the observed reference price
    /// crosses another $100 band. The floor never decreases through this path.
    pub fn observe_reference_price_usd_cents(&mut self, price_usd_cents: u64) {
        if price_usd_cents < INITIAL_PRICE_USD_CENTS { return; }
        let band = price_usd_cents / PRICE_BAND_USD_CENTS;
        if band <= self.highest_band_reached { return; }
        let bands_crossed = band - self.highest_band_reached;
        self.highest_band_reached = band;
        self.floor_usd_cents = self.floor_usd_cents.saturating_add(
            bands_crossed.saturating_mul(PRICE_FLOOR_STEP_USD_CENTS)
        );
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnecoinLedger {
    pub balances: BTreeMap<String, u128>,
    pub nonces: BTreeMap<String, u64>,
    pub members: BTreeMap<String, [u8; 32]>,
    pub total_issued_atoms: u128,
    pub join_remainder_atoms: u128,
    pub join_distribution_active: bool,
    pub price: OnecoinPricePolicy,
    pub reward_policy: ContributionRewardPolicy,
    pub reward_verifiers: BTreeMap<String, [u8; 32]>,
    pub rewarded_receipts: BTreeMap<[u8; 32], u128>,
}

impl Default for OnecoinLedger {
    fn default() -> Self {
        Self {
            balances: BTreeMap::new(),
            nonces: BTreeMap::new(),
            members: BTreeMap::new(),
            total_issued_atoms: 0,
            join_remainder_atoms: 0,
            join_distribution_active: true,
            price: OnecoinPricePolicy::default(),
            reward_policy: ContributionRewardPolicy::default(),
            reward_verifiers: BTreeMap::new(),
            rewarded_receipts: BTreeMap::new(),
        }
    }
}

impl OnecoinLedger {
    fn key(id: &AweId) -> String { id.to_hex() }

    pub fn balance_atoms(&self, id: &AweId) -> u128 {
        self.balances.get(&Self::key(id)).copied().unwrap_or(0)
    }

    pub fn member_count(&self) -> u64 { self.members.len() as u64 }

    /// Genesis allocation: each identity in the initial set receives exactly 10 ONECOIN.
    /// This operation is intended for a single deterministic genesis event.
    pub fn initialize_genesis(&mut self, members: &[AweId]) -> Result<(), String> {
        if members.is_empty() { return Err("genesis member set cannot be empty".into()); }
        if !self.members.is_empty() { return Err("genesis is already initialized".into()); }
        let mut unique = BTreeMap::new();
        for id in members { unique.insert(Self::key(id), *id.as_bytes()); }
        for (key, raw) in unique {
            self.members.insert(key.clone(), raw);
            self.balances.insert(key.clone(), INITIAL_GENESIS_ALLOCATION);
            self.nonces.insert(key, 0);
        }
        self.total_issued_atoms = INITIAL_GENESIS_ALLOCATION.saturating_mul(self.members.len() as u128);
        Ok(())
    }

    pub fn initialize_genesis_with_verifiers(
        &mut self,
        members: &[AweId],
        verifiers: &[[u8; 32]],
    ) -> Result<(), String> {
        self.initialize_genesis(members)?;
        for public_key in verifiers {
            let id = AweId::from_public_key(public_key);
            let key = Self::key(&id);
            if !self.members.contains_key(&key) {
                return Err("reward verifier must be a genesis member".into());
            }
            self.reward_verifiers.insert(key, *public_key);
        }
        Ok(())
    }

    pub fn authorize_reward_verifier(
        &mut self,
        authorizer_public_key: &[u8; 32],
        new_verifier_public_key: [u8; 32],
    ) -> Result<(), String> {
        let authorizer_id = AweId::from_public_key(authorizer_public_key);
        if !self.reward_verifiers.contains_key(&Self::key(&authorizer_id)) {
            return Err("caller is not an authorized reward verifier".into());
        }
        let id = AweId::from_public_key(&new_verifier_public_key);
        if !self.members.contains_key(&Self::key(&id)) {
            return Err("reward verifier must be a network member".into());
        }
        self.reward_verifiers.insert(Self::key(&id), new_verifier_public_key);
        Ok(())
    }

    /// Register a new member. One ONECOIN is created for the join-dividend
    /// and divided equally among all members after the join, including the new member.
    /// Once the atomic share would be zero, this distribution permanently stops.
    pub fn register_member(&mut self, id: &AweId) -> Result<JoinDistribution, String> {
        let key = Self::key(id);
        if self.members.contains_key(&key) { return Err("member already exists".into()); }
        self.members.insert(key.clone(), *id.as_bytes());
        self.balances.entry(key.clone()).or_insert(0);
        self.nonces.entry(key).or_insert(0);

        let distribution = JoinDistribution::calculate(*id, self.member_count());
        if self.join_distribution_active && distribution.active {
            let share = distribution.per_member_atoms;
            for member_key in self.members.keys().cloned().collect::<Vec<_>>() {
                let balance = self.balances.entry(member_key).or_insert(0);
                *balance = balance.saturating_add(share);
            }
            self.total_issued_atoms = self.total_issued_atoms.saturating_add(distribution.distributed_atoms);
            self.join_remainder_atoms = self.join_remainder_atoms.saturating_add(distribution.remainder_atoms);
        } else {
            self.join_distribution_active = false;
        }
        Ok(distribution)
    }

    pub fn mint_verified_contribution_reward(
        &mut self,
        signed_receipt: &SignedContributionReceipt,
    ) -> Result<ContributionReward, String> {
        let receipt_key = signed_receipt.hash();
        if !signed_receipt.verify() {
            return Err("invalid contribution receipt signature".into());
        }
        let verifier_id = AweId::from_public_key(&signed_receipt.verifier_public_key);
        let verifier_key = Self::key(&verifier_id);
        if self.reward_verifiers.get(&verifier_key)
            != Some(&signed_receipt.verifier_public_key)
        {
            return Err("receipt signer is not an authorized reward verifier".into());
        }
        if self.rewarded_receipts.contains_key(&receipt_key) {
            return Err("contribution receipt was already rewarded".into());
        }
        let node_key = Self::key(&signed_receipt.receipt.node);
        if !self.members.contains_key(&node_key) {
            return Err("reward recipient is not a member".into());
        }
        let reward_atoms = self.reward_policy.reward_for(&signed_receipt.receipt)?;
        let balance = self.balances.entry(node_key).or_insert(0);
        *balance = balance.saturating_add(reward_atoms);
        self.total_issued_atoms = self.total_issued_atoms.saturating_add(reward_atoms);
        self.rewarded_receipts.insert(receipt_key, reward_atoms);
        Ok(ContributionReward {
            node: signed_receipt.receipt.node.clone(),
            receipt_hash: receipt_key,
            reward_atoms,
        })
    }

    pub fn apply_transfer(
        &mut self,
        tx: &OnecoinTransaction,
        sender_public_key: &[u8; 32],
    ) -> Result<[u8; 32], String> {
        let sender_key = hex::encode(tx.sender);
        let recipient_key = hex::encode(tx.recipient);
        if !self.members.contains_key(&sender_key) || !self.members.contains_key(&recipient_key) {
            return Err("sender and recipient must be network members".into());
        }
        let expected_nonce = self.nonces.get(&sender_key).copied().unwrap_or(0);
        if tx.nonce != expected_nonce { return Err("invalid transaction nonce".into()); }
        if tx.sender != *AweId::from_public_key(sender_public_key).as_bytes() {
            return Err("sender public key does not match AWEID".into());
        }
        if !tx.verify(sender_public_key) { return Err("invalid ONECOIN signature".into()); }
        let sender_balance = self.balances.get(&sender_key).copied().unwrap_or(0);
        if sender_balance < tx.amount_atoms { return Err("insufficient ONECOIN balance".into()); }
        let sender_after = sender_balance - tx.amount_atoms;
        self.balances.insert(sender_key.clone(), sender_after);
        let recipient_balance = self.balances.get(&recipient_key).copied().unwrap_or(0);
        self.balances.insert(recipient_key, recipient_balance.saturating_add(tx.amount_atoms));
        self.nonces.insert(sender_key, expected_nonce + 1);
        Ok(tx.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awenet::ContributionReceipt;
    use crate::identity::{Identity, Username};

    fn id(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    #[test]
    fn genesis_gives_ten_each() {
        let a = id("a");
        let b = id("b");
        let mut l = OnecoinLedger::default();
        l.initialize_genesis(&[a.public.awe_id.clone(), b.public.awe_id.clone()]).unwrap();
        assert_eq!(l.balance_atoms(&a.public.awe_id), INITIAL_GENESIS_ALLOCATION);
        assert_eq!(l.balance_atoms(&b.public.awe_id), INITIAL_GENESIS_ALLOCATION);
    }

    #[test]
    fn join_one_coin_is_split_equally_and_remainder_is_preserved() {
        let a = id("a");
        let b = id("b");
        let mut l = OnecoinLedger::default();
        l.initialize_genesis(&[a.public.awe_id.clone()]).unwrap();
        let event = l.register_member(&b.public.awe_id).unwrap();
        assert_eq!(event.eligible_members, 2);
        assert_eq!(event.per_member_atoms, ATOMS_PER_COIN / 2);
        assert_eq!(l.balance_atoms(&a.public.awe_id), INITIAL_GENESIS_ALLOCATION + ATOMS_PER_COIN / 2);
        assert_eq!(l.balance_atoms(&b.public.awe_id), ATOMS_PER_COIN / 2);
    }

    #[test]
    fn join_distribution_eventually_stops_at_atomic_zero() {
        let event = JoinDistribution::calculate(
            id("a").public.awe_id,
            (ATOMS_PER_COIN + 1) as u64,
        );
        assert!(!event.active);
        assert_eq!(event.per_member_atoms, 0);
    }

    #[test]
    fn contribution_reward_is_deterministic_and_requires_verifier() {
        let a = id("a");
        let verifier = id("verifier");
        let mut l = OnecoinLedger::default();
        l.initialize_genesis_with_verifiers(
            &[a.public.awe_id.clone(), verifier.public.awe_id.clone()],
            &[verifier.public.public_key],
        ).unwrap();
        let r = ContributionReceipt {
            node: a.public.awe_id.clone(),
            storage_byte_hours: 100,
            relay_bytes: 0,
            bandwidth_bytes: 0,
            compute_units: 0,
            uptime_minutes: 10,
            period_start_unix: 1,
            period_end_unix: 2,
        };
        let signed = SignedContributionReceipt::new(&verifier, r.clone());
        let expected = l.reward_policy.reward_for(&r).unwrap();
        let before = l.balance_atoms(&a.public.awe_id);
        l.mint_verified_contribution_reward(&signed).unwrap();
        assert_eq!(l.balance_atoms(&a.public.awe_id), before + expected);
        assert!(l.mint_verified_contribution_reward(&signed).is_err());
    }

    #[test]
    fn signed_transfer_increments_nonce() {
        let a = id("a");
        let b = id("b");
        let mut l = OnecoinLedger::default();
        l.initialize_genesis(&[a.public.awe_id.clone(), b.public.awe_id.clone()]).unwrap();
        let tx = OnecoinTransaction::new(&a, 0, &b.public.awe_id, ATOMS_PER_COIN, None);
        l.apply_transfer(&tx, &a.public.public_key).unwrap();
        assert_eq!(l.balance_atoms(&a.public.awe_id), INITIAL_GENESIS_ALLOCATION - ATOMS_PER_COIN);
        assert_eq!(l.nonces[&a.public.awe_id.to_hex()], 1);
    }

    #[test]
    fn reward_policy_caps_issuance() {
        let policy = ContributionRewardPolicy {
            atoms_per_score: ATOMS_PER_COIN,
            max_reward_atoms: 2 * ATOMS_PER_COIN,
        };
        let r = ContributionReceipt {
            node: id("a").public.awe_id,
            storage_byte_hours: 10,
            relay_bytes: 0,
            bandwidth_bytes: 0,
            compute_units: 0,
            uptime_minutes: 10,
            period_start_unix: 1,
            period_end_unix: 2,
        };
        assert_eq!(policy.reward_for(&r).unwrap(), 2 * ATOMS_PER_COIN);
    }

    #[test]
    fn price_floor_only_moves_up_on_new_bands() {
        let mut p = OnecoinPricePolicy::default();
        p.observe_reference_price_usd_cents(10_000);
        assert_eq!(p.floor_usd_cents, 2_100);
        p.observe_reference_price_usd_cents(9_000);
        assert_eq!(p.floor_usd_cents, 2_100);
    }
}
