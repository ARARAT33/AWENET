//! Resource quota accounting and enforcement for node accounts.
//!
//! Deterministic ledger: tiers define limits, accounts track usage, and every
//! storage or transfer request gets an explicit allow or deny decision.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_ACCOUNTS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaTier {
    pub name: String,
    pub max_storage_bytes: u64,
    pub max_transfers_per_day: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub tier: String,
    pub used_storage_bytes: u64,
    pub transfers_today: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDenyReason {
    StorageExceeded,
    TransferLimitReached,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "detail", rename_all = "snake_case")]
pub enum QuotaDecision {
    Allowed { remaining_storage_bytes: u64 },
    Denied { reason: QuotaDenyReason },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QuotaError {
    #[error("tier or account name is empty")]
    EmptyName,
    #[error("storage limit must be greater than zero")]
    ZeroStorageLimit,
    #[error("unknown tier: {0}")]
    UnknownTier(String),
    #[error("unknown account: {0}")]
    UnknownAccount(String),
    #[error("duplicate account: {0}")]
    DuplicateAccount(String),
    #[error("quota ledger is full")]
    LedgerFull,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuotaLedger {
    tiers: BTreeMap<String, QuotaTier>,
    accounts: BTreeMap<String, Account>,
}

impl QuotaLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a quota tier.
    pub fn set_tier(
        &mut self,
        name: &str,
        max_storage_bytes: u64,
        max_transfers_per_day: u32,
    ) -> Result<(), QuotaError> {
        if name.is_empty() {
            return Err(QuotaError::EmptyName);
        }
        if max_storage_bytes == 0 {
            return Err(QuotaError::ZeroStorageLimit);
        }
        self.tiers.insert(
            name.to_string(),
            QuotaTier {
                name: name.to_string(),
                max_storage_bytes,
                max_transfers_per_day,
            },
        );
        Ok(())
    }

    /// Registers a new account under an existing tier.
    pub fn register(&mut self, account_id: &str, tier: &str) -> Result<(), QuotaError> {
        if account_id.is_empty() || tier.is_empty() {
            return Err(QuotaError::EmptyName);
        }
        if !self.tiers.contains_key(tier) {
            return Err(QuotaError::UnknownTier(tier.to_string()));
        }
        if self.accounts.contains_key(account_id) {
            return Err(QuotaError::DuplicateAccount(account_id.to_string()));
        }
        if self.accounts.len() >= MAX_ACCOUNTS {
            return Err(QuotaError::LedgerFull);
        }
        self.accounts.insert(
            account_id.to_string(),
            Account {
                id: account_id.to_string(),
                tier: tier.to_string(),
                used_storage_bytes: 0,
                transfers_today: 0,
            },
        );
        Ok(())
    }

    /// Records a storage request and returns the enforcement decision.
    pub fn record_storage(
        &mut self,
        account_id: &str,
        bytes: u64,
    ) -> Result<QuotaDecision, QuotaError> {
        let Some(account) = self.accounts.get_mut(account_id) else {
            return Err(QuotaError::UnknownAccount(account_id.to_string()));
        };
        let Some(tier) = self.tiers.get(&account.tier) else {
            return Err(QuotaError::UnknownTier(account.tier.clone()));
        };
        let allowed = match account.used_storage_bytes.checked_add(bytes) {
            Some(total) => total <= tier.max_storage_bytes,
            None => false,
        };
        if !allowed {
            return Ok(QuotaDecision::Denied {
                reason: QuotaDenyReason::StorageExceeded,
            });
        }
        account.used_storage_bytes += bytes;
        Ok(QuotaDecision::Allowed {
            remaining_storage_bytes: tier.max_storage_bytes - account.used_storage_bytes,
        })
    }

    /// Records a transfer request and returns the enforcement decision.
    pub fn record_transfer(&mut self, account_id: &str) -> Result<QuotaDecision, QuotaError> {
        let Some(account) = self.accounts.get_mut(account_id) else {
            return Err(QuotaError::UnknownAccount(account_id.to_string()));
        };
        let Some(tier) = self.tiers.get(&account.tier) else {
            return Err(QuotaError::UnknownTier(account.tier.clone()));
        };
        if account.transfers_today >= tier.max_transfers_per_day {
            return Ok(QuotaDecision::Denied {
                reason: QuotaDenyReason.TransferLimitReached,
            });
        }
        account.transfers_today += 1;
        Ok(QuotaDecision::Allowed {
            remaining_storage_bytes: tier.max_storage_bytes - account.used_storage_bytes,
        })
    }

    /// Returns storage usage as a percentage of the tier limit.
    pub fn usage_percent(&self, account_id: &str) -> Result<u32, QuotaError> {
        let Some(account) = self.accounts.get(account_id) else {
            return Err(QuotaError::UnknownAccount(account_id.to_string()));
        };
        let Some(tier) = self.tiers.get(&account.tier) else {
            return Err(QuotaError::UnknownTier(account.tier.clone()));
        };
        let percent =
            (account.used_storage_bytes as u128 * 100 / tier.max_storage_bytes as u128) as u32;
        Ok(percent.min(100))
    }

    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    pub fn tier_count(&self) -> usize {
        self.tiers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> QuotaLedger {
        let mut l = QuotaLedger::new();
        l.set_tier("basic", 100, 2).unwrap();
        l.register("alice", "basic").unwrap();
        l
    }

    #[test]
    fn validates_tiers_and_accounts() {
        let mut l = QuotaLedger::new();
        assert_eq!(l.set_tier("", 1, 1), Err(QuotaError::EmptyName));
        assert_eq!(l.set_tier("x", 0, 1), Err(QuotaError::ZeroStorageLimit));
        assert_eq!(
            l.register("a", "ghost"),
            Err(QuotaError::UnknownTier("ghost".into()))
        );
        assert_eq!(l.register("", "x"), Err(QuotaError::EmptyName));
        l.set_tier("x", 10, 1).unwrap();
        l.register("a", "x").unwrap();
        assert_eq!(
            l.register("a", "x"),
            Err(QuotaError::DuplicateAccount("a".into()))
        );
    }

    #[test]
    fn enforces_storage_and_transfers() {
        let mut l = ledger();
        assert_eq!(
            l.record_storage("alice", 60),
            Ok(QuotaDecision::Allowed {
                remaining_storage_bytes: 40
            })
        );
        assert_eq!(
            l.record_storage("alice", 50),
            Ok(QuotaDecision::Denied {
                reason: QuotaDenyReason::StorageExceeded
            })
        );
        assert_eq!(l.usage_percent("alice").unwrap(), 60);
        assert_eq!(
            l.record_transfer("alice").unwrap(),
            QuotaDecision::Allowed {
                remaining_storage_bytes: 40
            }
        );
        l.record_transfer("alice").unwrap();
        assert_eq!(
            l.record_transfer("alice").unwrap(),
            QuotaDecision::Denied {
                reason: QuotaDenyReason::TransferLimitReached
            }
        );
        assert_eq!(
            l.record_storage("bob", 1),
            Err(QuotaError::UnknownAccount("bob".into()))
        );
    }

    #[test]
    fn serializes_to_json() {
        let l = ledger();
        let json = serde_json::to_string(&l).unwrap();
        let back: QuotaLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(back.account_count(), 1);
        assert_eq!(back.tier_count(), 1);
    }
}
