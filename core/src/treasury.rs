//! ONECOIN treasury accounting for the AWENET economy.
//!
//! The treasury separates new issuance (rewards minted by founder decision
//! or verified contribution) from pure circulation (transfers, store
//! purchases, escrow movements), tracks escrow-held atoms, reports period
//! statistics and inflation risk bands, and reconciles its issuance total
//! against the OnecoinLedger so the economy never presents reserves that do
//! not exist.

use crate::onecoin::OnecoinLedger;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_TREASURY_ENTRIES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TreasuryError {
    #[error("treasury entries must carry a positive amount")]
    ZeroAmount,
    #[error("treasury log is full (max {MAX_TREASURY_ENTRIES} entries)")]
    LogFull,
    #[error("treasury issuance ({treasury_atoms} atoms) does not match the ledger ({ledger_atoms} atoms)")]
    LedgerMismatch {
        treasury_atoms: u128,
        ledger_atoms: u128,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreasuryFlow {
    GenesisIssued,
    TaskRewardIssued,
    ResourceRewardIssued,
    JoinDistributionIssued,
    PeerTransfer,
    StorePurchase,
    SellerIncome,
    EscrowHold,
    EscrowRelease,
    EscrowRefund,
    MarketplaceFee,
}

impl TreasuryFlow {
    /// True when this flow creates new ONECOIN instead of moving existing
    /// coins between members.
    pub fn is_issuance(self) -> bool {
        matches!(
            self,
            Self::GenesisIssued
                | Self::TaskRewardIssued
                | Self::ResourceRewardIssued
                | Self::JoinDistributionIssued
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreasuryEntry {
    pub sequence: u64,
    pub flow: TreasuryFlow,
    pub amount_atoms: u128,
    pub at_unix: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasurySummary {
    pub total_issued_atoms: u128,
    pub escrow_held_atoms: u128,
    pub circulating_atoms: u128,
    pub task_reward_atoms: u128,
    pub resource_reward_atoms: u128,
    pub store_volume_atoms: u128,
    pub seller_income_atoms: u128,
    pub transfer_volume_atoms: u128,
    pub fee_atoms: u128,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasuryPeriodStats {
    pub entries: usize,
    pub issued_atoms: u128,
    pub circulated_atoms: u128,
    pub store_volume_atoms: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InflationRisk {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Treasury {
    next_sequence: u64,
    entries: Vec<TreasuryEntry>,
}

impl Treasury {
    pub fn new() -> Self {
        Self {
            next_sequence: 1,
            entries: Vec::new(),
        }
    }

    pub fn record(
        &mut self,
        flow: TreasuryFlow,
        amount_atoms: u128,
        at_unix: u64,
    ) -> Result<u64, TreasuryError> {
        if amount_atoms == 0 {
            return Err(TreasuryError::ZeroAmount);
        }
        if self.entries.len() >= MAX_TREASURY_ENTRIES {
            return Err(TreasuryError::LogFull);
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.entries.push(TreasuryEntry {
            sequence,
            flow,
            amount_atoms,
            at_unix,
        });
        Ok(sequence)
    }

    pub fn entries(&self) -> &[TreasuryEntry] {
        &self.entries
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn summary(&self) -> TreasurySummary {
        let mut summary = TreasurySummary::default();
        for entry in &self.entries {
            match entry.flow {
                TreasuryFlow::GenesisIssued => {
                    summary.total_issued_atoms = summary
                        .total_issued_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::TaskRewardIssued => {
                    summary.total_issued_atoms = summary
                        .total_issued_atoms
                        .saturating_add(entry.amount_atoms);
                    summary.task_reward_atoms =
                        summary.task_reward_atoms.saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::ResourceRewardIssued => {
                    summary.total_issued_atoms = summary
                        .total_issued_atoms
                        .saturating_add(entry.amount_atoms);
                    summary.resource_reward_atoms = summary
                        .resource_reward_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::JoinDistributionIssued => {
                    summary.total_issued_atoms = summary
                        .total_issued_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::PeerTransfer => {
                    summary.transfer_volume_atoms = summary
                        .transfer_volume_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::StorePurchase => {
                    summary.store_volume_atoms = summary
                        .store_volume_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::SellerIncome => {
                    summary.seller_income_atoms = summary
                        .seller_income_atoms
                        .saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::EscrowHold => {
                    summary.escrow_held_atoms =
                        summary.escrow_held_atoms.saturating_add(entry.amount_atoms);
                }
                TreasuryFlow::EscrowRelease | TreasuryFlow::EscrowRefund => {
                    summary.escrow_held_atoms =
                        summary.escrow_held_atoms.saturating_sub(entry.amount_atoms);
                }
                TreasuryFlow::MarketplaceFee => {
                    summary.fee_atoms = summary.fee_atoms.saturating_add(entry.amount_atoms);
                }
            }
        }
        summary.circulating_atoms = summary
            .total_issued_atoms
            .saturating_sub(summary.escrow_held_atoms);
        summary
    }

    /// Aggregates only the entries whose timestamp falls in
    /// [start_unix, end_unix).
    pub fn period_stats(&self, start_unix: u64, end_unix: u64) -> TreasuryPeriodStats {
        let mut stats = TreasuryPeriodStats::default();
        for entry in &self.entries {
            if entry.at_unix >= start_unix && entry.at_unix < end_unix {
                stats.entries += 1;
                if entry.flow.is_issuance() {
                    stats.issued_atoms = stats.issued_atoms.saturating_add(entry.amount_atoms);
                } else {
                    stats.circulated_atoms =
                        stats.circulated_atoms.saturating_add(entry.amount_atoms);
                }
                if matches!(
                    entry.flow,
                    TreasuryFlow::StorePurchase | TreasuryFlow::SellerIncome
                ) {
                    stats.store_volume_atoms =
                        stats.store_volume_atoms.saturating_add(entry.amount_atoms);
                }
            }
        }
        stats
    }

    /// Issuance inside the window measured against the coins currently in
    /// circulation. High issuance against low circulation is inflationary.
    pub fn inflation_risk(&self, window_start_unix: u64, window_end_unix: u64) -> InflationRisk {
        let summary = self.summary();
        let stats = self.period_stats(window_start_unix, window_end_unix);
        if summary.circulating_atoms == 0 {
            return if stats.issued_atoms > 0 {
                InflationRisk::High
            } else {
                InflationRisk::Low
            };
        }
        let ratio_bps = stats.issued_atoms.saturating_mul(10_000) / summary.circulating_atoms;
        if ratio_bps < 500 {
            InflationRisk::Low
        } else if ratio_bps < 2_000 {
            InflationRisk::Medium
        } else {
            InflationRisk::High
        }
    }

    /// Rejects fake reserves: the treasury issuance total must equal the
    /// real ledger issuance total.
    pub fn reconcile(&self, ledger: &OnecoinLedger) -> Result<(), TreasuryError> {
        let issued = self.summary().total_issued_atoms;
        if issued != ledger.total_issued_atoms {
            return Err(TreasuryError::LedgerMismatch {
                treasury_atoms: issued,
                ledger_atoms: ledger.total_issued_atoms,
            });
        }
        Ok(())
    }
}

impl Default for Treasury {
    fn default() -> Self {
        Self::new()
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
    fn issuance_and_circulation_stay_separated() {
        let mut treasury = Treasury::new();
        treasury
            .record(TreasuryFlow::TaskRewardIssued, 100, 1)
            .unwrap();
        treasury
            .record(TreasuryFlow::ResourceRewardIssued, 40, 2)
            .unwrap();
        treasury.record(TreasuryFlow::PeerTransfer, 50, 3).unwrap();
        treasury.record(TreasuryFlow::StorePurchase, 20, 4).unwrap();
        treasury.record(TreasuryFlow::SellerIncome, 20, 5).unwrap();
        let summary = treasury.summary();
        assert_eq!(summary.total_issued_atoms, 140);
        assert_eq!(summary.task_reward_atoms, 100);
        assert_eq!(summary.resource_reward_atoms, 40);
        assert_eq!(summary.transfer_volume_atoms, 50);
        assert_eq!(summary.store_volume_atoms, 20);
        assert_eq!(summary.seller_income_atoms, 20);
        assert_eq!(summary.circulating_atoms, 140);
    }

    #[test]
    fn escrow_hold_and_release_track_circulating_atoms() {
        let mut treasury = Treasury::new();
        treasury
            .record(TreasuryFlow::GenesisIssued, 1_000, 1)
            .unwrap();
        treasury.record(TreasuryFlow::EscrowHold, 400, 2).unwrap();
        let held = treasury.summary();
        assert_eq!(held.escrow_held_atoms, 400);
        assert_eq!(held.circulating_atoms, 600);
        treasury
            .record(TreasuryFlow::EscrowRelease, 400, 3)
            .unwrap();
        let released = treasury.summary();
        assert_eq!(released.escrow_held_atoms, 0);
        assert_eq!(released.circulating_atoms, 1_000);
    }

    #[test]
    fn reconcile_rejects_fake_reserves() {
        let founder = identity("treasury-founder");
        let member = identity("treasury-member");
        let mut ledger = OnecoinLedger::default();
        ledger
            .initialize_genesis(&[founder.public.awe_id.clone(), member.public.awe_id.clone()])
            .unwrap();
        let mut treasury = Treasury::new();
        treasury.record(TreasuryFlow::GenesisIssued, 5, 1).unwrap();
        assert!(treasury.reconcile(&ledger).is_err());
        treasury.record(TreasuryFlow::GenesisIssued, 15, 2).unwrap();
        treasury.reconcile(&ledger).unwrap();
    }

    #[test]
    fn inflation_risk_bands() {
        let mut treasury = Treasury::new();
        treasury
            .record(TreasuryFlow::GenesisIssued, 10_000, 1)
            .unwrap();
        assert_eq!(treasury.inflation_risk(0, 100), InflationRisk::Low);
        treasury
            .record(TreasuryFlow::TaskRewardIssued, 600, 10)
            .unwrap();
        assert_eq!(treasury.inflation_risk(5, 100), InflationRisk::Medium);
        treasury
            .record(TreasuryFlow::TaskRewardIssued, 2_000, 20)
            .unwrap();
        assert_eq!(treasury.inflation_risk(5, 100), InflationRisk::High);
    }

    #[test]
    fn period_stats_respect_the_window() {
        let mut treasury = Treasury::new();
        treasury
            .record(TreasuryFlow::TaskRewardIssued, 10, 1)
            .unwrap();
        treasury.record(TreasuryFlow::PeerTransfer, 4, 2).unwrap();
        treasury.record(TreasuryFlow::StorePurchase, 6, 3).unwrap();
        let stats = treasury.period_stats(2, 3);
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.circulated_atoms, 4);
        assert_eq!(stats.store_volume_atoms, 0);
        let all = treasury.period_stats(0, 10);
        assert_eq!(all.entries, 3);
        assert_eq!(all.issued_atoms, 10);
        assert_eq!(all.circulated_atoms, 10);
        assert_eq!(all.store_volume_atoms, 6);
    }

    #[test]
    fn zero_amounts_are_rejected() {
        let mut treasury = Treasury::new();
        assert_eq!(
            treasury
                .record(TreasuryFlow::PeerTransfer, 0, 1)
                .unwrap_err(),
            TreasuryError::ZeroAmount
        );
        assert_eq!(treasury.entry_count(), 0);
    }
}
