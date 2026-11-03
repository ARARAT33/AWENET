//! Double-entry ledger with balanced journal entries.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. Every posted
//! journal entry must balance: total debits equal total credits. Account
//! balances are signed minor units, debits minus credits.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_ACCOUNTS: usize = 10_000;
pub const MAX_ENTRY_LINES: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LedgerError {
    #[error("ledger capacity must be between 1 and {MAX_ACCOUNTS}")]
    InvalidCapacity,
    #[error("ledger is full (max {MAX_ACCOUNTS} accounts)")]
    LedgerFull,
    #[error("account {0} already exists")]
    AccountExists(String),
    #[error("unknown account {0}")]
    UnknownAccount(String),
    #[error("entry name must not be empty")]
    EmptyEntryName,
    #[error("a journal entry needs between 2 and {MAX_ENTRY_LINES} lines")]
    InvalidLineCount,
    #[error("every line needs exactly one nonzero side")]
    InvalidAmount,
    #[error("journal entry is unbalanced: debits {0} do not equal credits {1}")]
    Unbalanced(u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub account: String,
    pub debit: u64,
    pub credit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    pub tick: u64,
    pub lines: Vec<Line>,
}

#[derive(Debug)]
pub struct Ledger {
    max_accounts: usize,
    next_entry_id: u64,
    accounts: BTreeMap<String, AccountKind>,
    entries: Vec<Entry>,
    balances: BTreeMap<String, i64>,
}

impl Ledger {
    pub fn new(max_accounts: usize) -> Result<Self, LedgerError> {
        if max_accounts == 0 || max_accounts > MAX_ACCOUNTS {
            return Err(LedgerError::InvalidCapacity);
        }
        Ok(Self {
            max_accounts,
            next_entry_id: 1,
            accounts: BTreeMap::new(),
            entries: Vec::new(),
            balances: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn add_account(&mut self, name: &str, kind: AccountKind) -> Result<(), LedgerError> {
        if self.accounts.len() >= self.max_accounts {
            return Err(LedgerError::LedgerFull);
        }
        if self.accounts.contains_key(name) {
            return Err(LedgerError::AccountExists(name.to_string()));
        }
        self.accounts.insert(name.to_string(), kind);
        self.balances.insert(name.to_string(), 0);
        Ok(())
    }

    pub fn account_kind(&self, name: &str) -> Result<AccountKind, LedgerError> {
        self.accounts
            .get(name)
            .copied()
            .ok_or_else(|| LedgerError::UnknownAccount(name.to_string()))
    }

    /// Posts a balanced journal entry and returns its id.
    pub fn post(&mut self, name: &str, tick: u64, lines: Vec<Line>) -> Result<u64, LedgerError> {
        if name.is_empty() {
            return Err(LedgerError::EmptyEntryName);
        }
        if lines.len() < 2 || lines.len() > MAX_ENTRY_LINES {
            return Err(LedgerError::InvalidLineCount);
        }
        let mut debits = 0u64;
        let mut credits = 0u64;
        for line in &lines {
            if (line.debit > 0) == (line.credit > 0) {
                return Err(LedgerError::InvalidAmount);
            }
            if !self.accounts.contains_key(&line.account) {
                return Err(LedgerError::UnknownAccount(line.account.clone()));
            }
            debits = debits.saturating_add(line.debit);
            credits = credits.saturating_add(line.credit);
        }
        if debits != credits {
            return Err(LedgerError::Unbalanced(debits, credits));
        }
        for line in &lines {
            let balance = self.balances.get_mut(&line.account).expect("checked above");
            if line.debit > 0 {
                *balance += i64::try_from(line.debit).expect("positive fits i64");
            } else {
                *balance -= i64::try_from(line.credit).expect("positive fits i64");
            }
        }
        let id = self.next_entry_id;
        self.next_entry_id += 1;
        self.entries.push(Entry {
            id,
            name: name.to_string(),
            tick,
            lines,
        });
        Ok(id)
    }

    pub fn balance(&self, account: &str) -> Result<i64, LedgerError> {
        self.balances
            .get(account)
            .copied()
            .ok_or_else(|| LedgerError::UnknownAccount(account.to_string()))
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entry(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Signed balances of every account, ordered by account name.
    pub fn trial_balance(&self) -> Vec<(&str, i64)> {
        self.balances
            .iter()
            .map(|(account, balance)| (account.as_str(), *balance))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(account: &str, debit: u64, credit: u64) -> Line {
        Line {
            account: account.to_string(),
            debit,
            credit,
        }
    }

    fn ledger() -> Ledger {
        let mut ledger = Ledger::new(8).unwrap();
        ledger.add_account("wallet", AccountKind::Asset).unwrap();
        ledger.add_account("fees", AccountKind::Revenue).unwrap();
        ledger
    }

    #[test]
    fn validates_capacity_and_duplicates() {
        assert_eq!(Ledger::new(0).unwrap_err(), LedgerError::InvalidCapacity);
        assert_eq!(
            Ledger::new(MAX_ACCOUNTS + 1).unwrap_err(),
            LedgerError::InvalidCapacity
        );
        let mut ledger = ledger();
        assert_eq!(
            ledger
                .add_account("wallet", AccountKind::Asset)
                .unwrap_err(),
            LedgerError::AccountExists("wallet".into())
        );
    }

    #[test]
    fn ledger_full_is_rejected() {
        let mut ledger = Ledger::new(1).unwrap();
        ledger.add_account("wallet", AccountKind::Asset).unwrap();
        assert_eq!(
            ledger
                .add_account("fees", AccountKind::Revenue)
                .unwrap_err(),
            LedgerError::LedgerFull
        );
        assert_eq!(ledger.len(), 1);
        assert!(!ledger.is_empty());
    }

    #[test]
    fn rejects_invalid_entries() {
        let mut ledger = ledger();
        assert_eq!(
            ledger
                .post("", 0, vec![line("wallet", 10, 0), line("fees", 0, 10)])
                .unwrap_err(),
            LedgerError::EmptyEntryName
        );
        assert_eq!(
            ledger
                .post("solo", 0, vec![line("wallet", 10, 0)])
                .unwrap_err(),
            LedgerError::InvalidLineCount
        );
        assert_eq!(
            ledger
                .post("zero", 0, vec![line("wallet", 0, 0), line("fees", 0, 0)])
                .unwrap_err(),
            LedgerError::InvalidAmount
        );
        assert_eq!(
            ledger
                .post("both", 0, vec![line("wallet", 5, 5), line("fees", 0, 10)])
                .unwrap_err(),
            LedgerError::InvalidAmount
        );
        assert_eq!(
            ledger
                .post(
                    "unknown",
                    0,
                    vec![line("ghost", 10, 0), line("fees", 0, 10)]
                )
                .unwrap_err(),
            LedgerError::UnknownAccount("ghost".into())
        );
    }

    #[test]
    fn rejects_unbalanced_entries() {
        let mut ledger = ledger();
        assert_eq!(
            ledger
                .post(
                    "lop-sided",
                    1,
                    vec![line("wallet", 30, 0), line("fees", 0, 20)]
                )
                .unwrap_err(),
            LedgerError::Unbalanced(30, 20)
        );
        assert_eq!(ledger.entry_count(), 0);
    }

    #[test]
    fn balanced_entries_move_balances() {
        let mut ledger = ledger();
        let id = ledger
            .post(
                "charge-fee",
                7,
                vec![line("wallet", 30, 0), line("fees", 0, 30)],
            )
            .unwrap();
        assert_eq!(ledger.balance("wallet").unwrap(), 30);
        assert_eq!(ledger.balance("fees").unwrap(), -30);
        assert_eq!(ledger.entry(id).unwrap().tick, 7);
        assert_eq!(ledger.entry_count(), 1);

        ledger
            .post(
                "refund-fee",
                9,
                vec![line("wallet", 0, 5), line("fees", 5, 0)],
            )
            .unwrap();
        assert_eq!(ledger.balance("wallet").unwrap(), 25);
        assert_eq!(ledger.balance("fees").unwrap(), -25);
        assert_eq!(ledger.account_kind("fees").unwrap(), AccountKind::Revenue);
        assert_eq!(
            ledger.balance("ghost").unwrap_err(),
            LedgerError::UnknownAccount("ghost".into())
        );
    }

    #[test]
    fn trial_balance_lists_every_account() {
        let mut ledger = ledger();
        ledger
            .post(
                "charge-fee",
                1,
                vec![line("fees", 0, 12), line("wallet", 12, 0)],
            )
            .unwrap();
        let trial = ledger.trial_balance();
        assert_eq!(trial.len(), 2);
        assert_eq!(trial[0], ("fees", -12));
        assert_eq!(trial[1], ("wallet", 12));
    }
}
