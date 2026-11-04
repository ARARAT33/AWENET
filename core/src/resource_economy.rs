//! Resource contribution economy for AWENET.
//!
//! Participants volunteer device resources under explicit consent limits
//! per settlement period. Rewards are computed from measured, multi-signal
//! usage (never from the node's own claim alone), weighted by a reliability
//! coefficient, and clearly separated from a small availability reward. The
//! founder owns the rate table; nobody else can change it. Violations of
//! the consented limits reduce future rewards.

use crate::founder::{FounderAction, FounderAuthority, FounderError};
use crate::identity::{AweId, Identity};
use crate::onecoin::OnecoinLedger;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_GRANTS: usize = 10_000;
pub const MAX_SETTLEMENT_LOG: usize = 10_000;
pub const REQUIRED_USAGE_SIGNALS: usize = 2;
pub const VIOLATION_PENALTY_BPS: u16 = 1_000;
pub const MIN_RELIABILITY_BPS: u16 = 1_000;
pub const FULL_RELIABILITY_BPS: u16 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ResourceEconomyError {
    #[error(transparent)]
    Founder(#[from] FounderError),
    #[error("resource grant limits must offer at least one resource")]
    InvalidLimits,
    #[error("contribution grant registry is full (max {MAX_GRANTS} grants)")]
    RegistryFull,
    #[error("no active contribution grant for this node")]
    NoGrant,
    #[error("usage period is invalid")]
    InvalidPeriod,
    #[error("at least {REQUIRED_USAGE_SIGNALS} independent usage signals are required")]
    InsufficientSignals,
    #[error("this contribution period was already settled")]
    AlreadySettled,
    #[error("settlement log is full (max {MAX_SETTLEMENT_LOG} settlements)")]
    SettlementLogFull,
    #[error("ledger rejected the resource reward: {0}")]
    Ledger(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    Cpu,
    Gpu,
    Ram,
    Storage,
    Bandwidth,
    Uptime,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Ram => "ram",
            Self::Storage => "storage",
            Self::Bandwidth => "bandwidth",
            Self::Uptime => "uptime",
        }
    }
}

/// What a node consents to contribute, as caps per settlement period. The
/// owner can change or withdraw the grant at any time.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLimits {
    pub cpu_core_ticks: u64,
    pub gpu_unit_ticks: u64,
    pub ram_mb_hours: u128,
    pub storage_gb_hours: u128,
    pub bandwidth_gb: u64,
    pub uptime_hours: u64,
}

impl ResourceLimits {
    pub fn validate(&self) -> Result<(), ResourceEconomyError> {
        if self.cpu_core_ticks == 0
            && self.gpu_unit_ticks == 0
            && self.ram_mb_hours == 0
            && self.storage_gb_hours == 0
            && self.bandwidth_gb == 0
            && self.uptime_hours == 0
        {
            return Err(ResourceEconomyError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeasuredUsage {
    pub cpu_core_ticks: u64,
    pub gpu_unit_ticks: u64,
    pub ram_mb_hours: u128,
    pub storage_gb_hours: u128,
    pub bandwidth_gb: u64,
    pub uptime_hours: u64,
}

/// Independent measurements of the same contribution period: the node's own
/// signed report, the network's own measurements and the delivered results
/// of the work actually served.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageEvidence {
    pub node_report: Option<MeasuredUsage>,
    pub network_measurement: Option<MeasuredUsage>,
    pub delivered_results: Option<MeasuredUsage>,
}

impl UsageEvidence {
    pub fn signal_count(&self) -> usize {
        [
            &self.node_report,
            &self.network_measurement,
            &self.delivered_results,
        ]
        .into_iter()
        .filter(|signal| signal.is_some())
        .count()
    }

    /// Conservative effective usage: the minimum across the available
    /// independent signals, so a single inflated report cannot raise the
    /// reward.
    pub fn effective_usage(&self) -> Option<MeasuredUsage> {
        if self.signal_count() < REQUIRED_USAGE_SIGNALS {
            return None;
        }
        let signals: Vec<&MeasuredUsage> = [
            &self.node_report,
            &self.network_measurement,
            &self.delivered_results,
        ]
        .into_iter()
        .filter_map(|signal| signal.as_ref())
        .collect();
        Some(MeasuredUsage {
            cpu_core_ticks: signals.iter().map(|s| s.cpu_core_ticks).min().unwrap_or(0),
            gpu_unit_ticks: signals.iter().map(|s| s.gpu_unit_ticks).min().unwrap_or(0),
            ram_mb_hours: signals.iter().map(|s| s.ram_mb_hours).min().unwrap_or(0),
            storage_gb_hours: signals
                .iter()
                .map(|s| s.storage_gb_hours)
                .min()
                .unwrap_or(0),
            bandwidth_gb: signals.iter().map(|s| s.bandwidth_gb).min().unwrap_or(0),
            uptime_hours: signals.iter().map(|s| s.uptime_hours).min().unwrap_or(0),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRates {
    pub atoms_per_cpu_core_tick: u128,
    pub atoms_per_gpu_unit_tick: u128,
    pub atoms_per_ram_mb_hour: u128,
    pub atoms_per_storage_gb_hour: u128,
    pub atoms_per_bandwidth_gb: u128,
    pub atoms_per_uptime_hour: u128,
    pub availability_atoms_per_hour: u128,
}

impl Default for ResourceRates {
    fn default() -> Self {
        Self {
            atoms_per_cpu_core_tick: 1_000,
            atoms_per_gpu_unit_tick: 2_000,
            atoms_per_ram_mb_hour: 10,
            atoms_per_storage_gb_hour: 1_000,
            atoms_per_bandwidth_gb: 5_000,
            atoms_per_uptime_hour: 100,
            availability_atoms_per_hour: 50,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionGrant {
    pub node: AweId,
    pub limits: ResourceLimits,
    pub granted_at_unix: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceSettlement {
    pub node: AweId,
    pub period_start_unix: u64,
    pub period_end_unix: u64,
    pub usage_atoms: u128,
    pub availability_atoms: u128,
    pub total_atoms: u128,
    pub reliability_bps: u16,
    pub violations: Vec<ResourceKind>,
    pub settlement_id: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceEconomy {
    rates: ResourceRates,
    grants: BTreeMap<String, ContributionGrant>,
    violation_counts: BTreeMap<String, u32>,
    settlements: Vec<ResourceSettlement>,
    settled: BTreeSet<[u8; 32]>,
}

impl ResourceEconomy {
    pub fn new(rates: ResourceRates) -> Self {
        Self {
            rates,
            grants: BTreeMap::new(),
            violation_counts: BTreeMap::new(),
            settlements: Vec::new(),
            settled: BTreeSet::new(),
        }
    }

    pub fn rates(&self) -> &ResourceRates {
        &self.rates
    }

    /// Only the founder may change the resource reward rates.
    pub fn set_rates(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        rates: ResourceRates,
        at_unix: u64,
    ) -> Result<(), ResourceEconomyError> {
        authority.authorize(
            actor_public_key,
            FounderAction::SetResourceRates,
            at_unix,
            "update resource rates",
        )?;
        self.rates = rates;
        Ok(())
    }

    /// Registers or replaces the contribution grant of a node. Only the
    /// node owner can do this, and only for their own identity.
    pub fn grant(
        &mut self,
        owner: &Identity,
        limits: ResourceLimits,
        now_unix: u64,
    ) -> Result<(), ResourceEconomyError> {
        limits.validate()?;
        let key = owner.public.awe_id.to_hex();
        if !self.grants.contains_key(&key) && self.grants.len() >= MAX_GRANTS {
            return Err(ResourceEconomyError::RegistryFull);
        }
        self.grants.insert(
            key,
            ContributionGrant {
                node: owner.public.awe_id.clone(),
                limits,
                granted_at_unix: now_unix,
                active: true,
            },
        );
        Ok(())
    }

    pub fn update_limits(
        &mut self,
        owner: &Identity,
        limits: ResourceLimits,
        now_unix: u64,
    ) -> Result<(), ResourceEconomyError> {
        limits.validate()?;
        let key = owner.public.awe_id.to_hex();
        let grant = self
            .grants
            .get_mut(&key)
            .ok_or(ResourceEconomyError::NoGrant)?;
        grant.limits = limits;
        grant.granted_at_unix = now_unix;
        Ok(())
    }

    /// The owner can stop participating at any time; the network must
    /// respect it immediately.
    pub fn withdraw(&mut self, owner: &Identity) -> Result<(), ResourceEconomyError> {
        let grant = self
            .grants
            .get_mut(&owner.public.awe_id.to_hex())
            .ok_or(ResourceEconomyError::NoGrant)?;
        grant.active = false;
        Ok(())
    }

    pub fn resume(&mut self, owner: &Identity) -> Result<(), ResourceEconomyError> {
        let grant = self
            .grants
            .get_mut(&owner.public.awe_id.to_hex())
            .ok_or(ResourceEconomyError::NoGrant)?;
        grant.active = true;
        Ok(())
    }

    pub fn grant_for(&self, node: &AweId) -> Option<&ContributionGrant> {
        self.grants.get(&node.to_hex())
    }

    pub fn settlement_log(&self) -> &[ResourceSettlement] {
        &self.settlements
    }

    /// Settles one contribution period for one node: verifies the grant,
    /// requires independent usage signals, clamps usage to the consented
    /// limits while recording violations, applies the reliability
    /// coefficient, mints the reward exactly once, and separates usage pay
    /// from the small availability pay.
    pub fn settle(
        &mut self,
        ledger: &mut OnecoinLedger,
        verifier: &Identity,
        node: &AweId,
        evidence: &UsageEvidence,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> Result<ResourceSettlement, ResourceEconomyError> {
        if period_end_unix <= period_start_unix {
            return Err(ResourceEconomyError::InvalidPeriod);
        }
        let key = node.to_hex();
        let grant = self.grants.get(&key).ok_or(ResourceEconomyError::NoGrant)?;
        if !grant.active {
            return Err(ResourceEconomyError::NoGrant);
        }
        let limits = grant.limits.clone();
        let effective = evidence
            .effective_usage()
            .ok_or(ResourceEconomyError::InsufficientSignals)?;

        let mut violations = Vec::new();
        if effective.cpu_core_ticks > limits.cpu_core_ticks {
            violations.push(ResourceKind::Cpu);
        }
        if effective.gpu_unit_ticks > limits.gpu_unit_ticks {
            violations.push(ResourceKind::Gpu);
        }
        if effective.ram_mb_hours > limits.ram_mb_hours {
            violations.push(ResourceKind::Ram);
        }
        if effective.storage_gb_hours > limits.storage_gb_hours {
            violations.push(ResourceKind::Storage);
        }
        if effective.bandwidth_gb > limits.bandwidth_gb {
            violations.push(ResourceKind::Bandwidth);
        }
        if effective.uptime_hours > limits.uptime_hours {
            violations.push(ResourceKind::Uptime);
        }
        let clamped = MeasuredUsage {
            cpu_core_ticks: effective.cpu_core_ticks.min(limits.cpu_core_ticks),
            gpu_unit_ticks: effective.gpu_unit_ticks.min(limits.gpu_unit_ticks),
            ram_mb_hours: effective.ram_mb_hours.min(limits.ram_mb_hours),
            storage_gb_hours: effective.storage_gb_hours.min(limits.storage_gb_hours),
            bandwidth_gb: effective.bandwidth_gb.min(limits.bandwidth_gb),
            uptime_hours: effective.uptime_hours.min(limits.uptime_hours),
        };

        let past_violations = self.violation_counts.get(&key).copied().unwrap_or(0);
        let total_violations = past_violations.saturating_add(violations.len() as u32);
        let max_penalty = FULL_RELIABILITY_BPS - MIN_RELIABILITY_BPS;
        let penalty = (total_violations.saturating_mul(VIOLATION_PENALTY_BPS as u32))
            .min(max_penalty as u32) as u16;
        let reliability_bps = FULL_RELIABILITY_BPS - penalty;

        let rates = self.rates.clone();
        let cpu_atoms =
            (clamped.cpu_core_ticks as u128).saturating_mul(rates.atoms_per_cpu_core_tick);
        let gpu_atoms =
            (clamped.gpu_unit_ticks as u128).saturating_mul(rates.atoms_per_gpu_unit_tick);
        let ram_atoms = clamped
            .ram_mb_hours
            .saturating_mul(rates.atoms_per_ram_mb_hour);
        let storage_atoms = clamped
            .storage_gb_hours
            .saturating_mul(rates.atoms_per_storage_gb_hour);
        let bandwidth_atoms =
            (clamped.bandwidth_gb as u128).saturating_mul(rates.atoms_per_bandwidth_gb);
        let uptime_atoms =
            (clamped.uptime_hours as u128).saturating_mul(rates.atoms_per_uptime_hour);
        let usage_atoms = cpu_atoms
            .saturating_add(gpu_atoms)
            .saturating_add(ram_atoms)
            .saturating_add(storage_atoms)
            .saturating_add(bandwidth_atoms)
            .saturating_add(uptime_atoms)
            .saturating_mul(reliability_bps as u128)
            / 10_000;
        // Availability pay is a separate, smaller reward for keeping a
        // demanded resource reachable, distinct from the usage payment.
        let availability_atoms = rates
            .availability_atoms_per_hour
            .saturating_mul(clamped.uptime_hours as u128)
            .saturating_mul(reliability_bps as u128)
            / 10_000;
        let total_atoms = usage_atoms.saturating_add(availability_atoms);

        let settlement_id = {
            let mut input = Vec::new();
            input.extend_from_slice(node.as_bytes());
            input.extend_from_slice(&period_start_unix.to_be_bytes());
            input.extend_from_slice(&period_end_unix.to_be_bytes());
            *blake3::hash(&input).as_bytes()
        };
        if self.settled.contains(&settlement_id) {
            return Err(ResourceEconomyError::AlreadySettled);
        }
        if self.settlements.len() >= MAX_SETTLEMENT_LOG {
            return Err(ResourceEconomyError::SettlementLogFull);
        }
        if total_atoms > 0 {
            ledger
                .mint_approved_reward(
                    &verifier.public.public_key,
                    node,
                    total_atoms,
                    settlement_id,
                )
                .map_err(ResourceEconomyError::Ledger)?;
        }
        if !violations.is_empty() {
            self.violation_counts.insert(key, total_violations);
        }
        let settlement = ResourceSettlement {
            node: node.clone(),
            period_start_unix,
            period_end_unix,
            usage_atoms,
            availability_atoms,
            total_atoms,
            reliability_bps,
            violations,
            settlement_id,
        };
        self.settled.insert(settlement_id);
        self.settlements.push(settlement.clone());
        Ok(settlement)
    }
}

impl Default for ResourceEconomy {
    fn default() -> Self {
        Self::new(ResourceRates::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::onecoin::ATOMS_PER_COIN;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn ledger(verifier: &Identity, nodes: &[AweId]) -> OnecoinLedger {
        let mut ledger = OnecoinLedger::default();
        let mut members = vec![verifier.public.awe_id.clone()];
        members.extend(nodes.iter().cloned());
        ledger
            .initialize_genesis_with_verifiers(&members, &[verifier.public.public_key])
            .unwrap();
        ledger
    }

    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_core_ticks: 100,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: 10,
            uptime_hours: 24,
        }
    }

    fn evidence(cpu: u64, uptime: u64) -> UsageEvidence {
        let usage = MeasuredUsage {
            cpu_core_ticks: cpu,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: 0,
            uptime_hours: uptime,
        };
        UsageEvidence {
            node_report: Some(usage.clone()),
            network_measurement: Some(usage.clone()),
            delivered_results: Some(usage),
        }
    }

    #[test]
    fn limits_must_offer_something() {
        assert!(ResourceLimits::default().validate().is_err());
        assert!(limits().validate().is_ok());
    }

    #[test]
    fn only_the_founder_changes_rates() {
        let founder = identity("founder");
        let member = identity("member");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut economy = ResourceEconomy::default();
        let cheap = ResourceRates {
            atoms_per_cpu_core_tick: 1,
            atoms_per_gpu_unit_tick: 1,
            atoms_per_ram_mb_hour: 1,
            atoms_per_storage_gb_hour: 1,
            atoms_per_bandwidth_gb: 1,
            atoms_per_uptime_hour: 1,
            availability_atoms_per_hour: 1,
        };
        assert!(matches!(
            economy
                .set_rates(&mut authority, &member.public.public_key, cheap.clone(), 1)
                .unwrap_err(),
            ResourceEconomyError::Founder(FounderError::NotFounder)
        ));
        assert_eq!(economy.rates().atoms_per_cpu_core_tick, 1_000);
        economy
            .set_rates(&mut authority, &founder.public.public_key, cheap.clone(), 2)
            .unwrap();
        assert_eq!(economy.rates().atoms_per_cpu_core_tick, 1);
    }

    #[test]
    fn owner_controls_participation() {
        let owner = identity("owner");
        let mut economy = ResourceEconomy::default();
        economy.grant(&owner, limits(), 1).unwrap();
        assert!(economy.grant_for(&owner.public.awe_id).unwrap().active);
        economy.withdraw(&owner).unwrap();
        assert!(!economy.grant_for(&owner.public.awe_id).unwrap().active);
        economy.resume(&owner).unwrap();
        assert!(economy.grant_for(&owner.public.awe_id).unwrap().active);
        let other = identity("other");
        assert!(economy.withdraw(&other).is_err());
        let tighter = ResourceLimits {
            cpu_core_ticks: 1,
            ..ResourceLimits::default()
        };
        economy.update_limits(&owner, tighter, 5).unwrap();
        assert_eq!(
            economy
                .grant_for(&owner.public.awe_id)
                .unwrap()
                .limits
                .cpu_core_ticks,
            1
        );
    }

    #[test]
    fn rewards_follow_measured_usage_not_claims() {
        let founder = identity("founder");
        let heavy = identity("heavy");
        let light = identity("light");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(
            &founder,
            &[heavy.public.awe_id.clone(), light.public.awe_id.clone()],
        );
        economy.grant(&heavy, limits(), 1).unwrap();
        economy.grant(&light, limits(), 1).unwrap();
        economy
            .settle(
                &mut ledger,
                &founder,
                &heavy.public.awe_id,
                &evidence(80, 0),
                0,
                3_600,
            )
            .unwrap();
        economy
            .settle(
                &mut ledger,
                &founder,
                &light.public.awe_id,
                &evidence(20, 0),
                0,
                3_600,
            )
            .unwrap();
        let log = economy.settlement_log();
        assert_eq!(log[0].usage_atoms, 80_000);
        assert_eq!(log[1].usage_atoms, 20_000);
        assert_eq!(log[0].availability_atoms, 0);
        assert_eq!(
            ledger.balance_atoms(&heavy.public.awe_id),
            10 * ATOMS_PER_COIN + 80_000
        );
        assert_eq!(
            ledger.balance_atoms(&light.public.awe_id),
            10 * ATOMS_PER_COIN + 20_000
        );
    }

    #[test]
    fn a_node_report_alone_cannot_earn() {
        let founder = identity("founder");
        let node = identity("node");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(&founder, std::slice::from_ref(&node.public.awe_id));
        economy.grant(&node, limits(), 1).unwrap();
        let mut single = evidence(50, 1);
        single.network_measurement = None;
        single.delivered_results = None;
        assert!(matches!(
            economy
                .settle(
                    &mut ledger,
                    &founder,
                    &node.public.awe_id,
                    &single,
                    0,
                    3_600
                )
                .unwrap_err(),
            ResourceEconomyError::InsufficientSignals
        ));
    }

    #[test]
    fn inflated_claims_are_capped_and_violations_reduce_reliability() {
        let founder = identity("founder");
        let node = identity("node");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(&founder, std::slice::from_ref(&node.public.awe_id));
        economy.grant(&node, limits(), 1).unwrap();
        let beyond = evidence(150, 0);
        let settlement = economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &beyond,
                0,
                3_600,
            )
            .unwrap();
        assert_eq!(settlement.violations, vec![ResourceKind::Cpu]);
        assert_eq!(settlement.reliability_bps, 9_000);
        assert_eq!(settlement.usage_atoms, 90_000);
        let second = economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &evidence(10, 0),
                3_600,
                7_200,
            )
            .unwrap();
        assert_eq!(second.reliability_bps, 9_000);
        let third = economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &beyond,
                7_200,
                10_800,
            )
            .unwrap();
        assert_eq!(third.reliability_bps, 8_000);
    }

    #[test]
    fn a_contribution_period_settles_exactly_once() {
        let founder = identity("founder");
        let node = identity("node");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(&founder, std::slice::from_ref(&node.public.awe_id));
        economy.grant(&node, limits(), 1).unwrap();
        economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &evidence(10, 1),
                0,
                3_600,
            )
            .unwrap();
        assert!(matches!(
            economy
                .settle(
                    &mut ledger,
                    &founder,
                    &node.public.awe_id,
                    &evidence(10, 1),
                    0,
                    3_600
                )
                .unwrap_err(),
            ResourceEconomyError::AlreadySettled
        ));
        assert_eq!(
            ledger.balance_atoms(&node.public.awe_id),
            10 * ATOMS_PER_COIN + 10_150
        );
    }

    #[test]
    fn availability_pay_is_separate_from_usage_pay() {
        let founder = identity("founder");
        let node = identity("node");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(&founder, std::slice::from_ref(&node.public.awe_id));
        economy.grant(&node, limits(), 1).unwrap();
        let settlement = economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &evidence(0, 2),
                0,
                3_600,
            )
            .unwrap();
        assert_eq!(settlement.usage_atoms, 200);
        assert_eq!(settlement.availability_atoms, 100);
        assert_eq!(settlement.total_atoms, 300);
        assert!(settlement.violations.is_empty());
    }

    #[test]
    fn withdrawn_nodes_stop_earning() {
        let founder = identity("founder");
        let node = identity("node");
        let mut economy = ResourceEconomy::default();
        let mut ledger = ledger(&founder, std::slice::from_ref(&node.public.awe_id));
        economy.grant(&node, limits(), 1).unwrap();
        economy.withdraw(&node).unwrap();
        assert!(matches!(
            economy
                .settle(
                    &mut ledger,
                    &founder,
                    &node.public.awe_id,
                    &evidence(10, 1),
                    0,
                    3_600
                )
                .unwrap_err(),
            ResourceEconomyError::NoGrant
        ));
        assert_eq!(
            ledger.balance_atoms(&node.public.awe_id),
            10 * ATOMS_PER_COIN
        );
    }
}
