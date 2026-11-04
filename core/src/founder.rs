//! Founder authority for the AWENET economy.
//!
//! ONECOIN issuance, AWENETwork task publication, submission review and the
//! official price are exclusive rights of the AWENET founder. This module
//! makes that exclusivity a backend rule instead of a UI promise: every
//! protected operation must first pass `FounderAuthority::authorize`, which
//! compares the caller's public key with the founder key installed once at
//! genesis. There is deliberately no API to replace, delegate or edit the
//! founder identity, so the right cannot be acquired through profile fields
//! or ordinary API calls.

use crate::identity::{AweId, Identity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_FOUNDER_LOG: usize = 10_000;
pub const MAX_ACTION_DETAIL_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum FounderError {
    #[error("only the AWENET founder may perform this operation")]
    NotFounder,
    #[error("founder audit log is full (max {MAX_FOUNDER_LOG} entries)")]
    LogFull,
    #[error("action detail exceeds {MAX_ACTION_DETAIL_CHARS} characters")]
    DetailTooLong,
    #[error("founder audit sequence is exhausted")]
    SequenceExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FounderAction {
    PublishTask,
    ModifyTask,
    CloseTask,
    ReviewSubmission,
    SetOfficialPrice,
    SetResourceRates,
    ManageTreasury,
    SuspendAccount,
    SettleDispute,
}

impl FounderAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublishTask => "publish_task",
            Self::ModifyTask => "modify_task",
            Self::CloseTask => "close_task",
            Self::ReviewSubmission => "review_submission",
            Self::SetOfficialPrice => "set_official_price",
            Self::SetResourceRates => "set_resource_rates",
            Self::ManageTreasury => "manage_treasury",
            Self::SuspendAccount => "suspend_account",
            Self::SettleDispute => "settle_dispute",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderAuditEntry {
    pub sequence: u64,
    pub action: FounderAction,
    pub actor: AweId,
    pub at_unix: u64,
    pub detail: String,
}

/// Single-founder authority. The founder key is installed once at genesis
/// and is immutable afterwards.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderAuthority {
    founder: AweId,
    founder_public_key: [u8; 32],
    installed_at_unix: u64,
    next_sequence: u64,
    log: Vec<FounderAuditEntry>,
}

impl FounderAuthority {
    pub fn install(founder: &Identity, at_unix: u64) -> Self {
        Self {
            founder: founder.public.awe_id.clone(),
            founder_public_key: founder.public.public_key,
            installed_at_unix: at_unix,
            next_sequence: 1,
            log: Vec::new(),
        }
    }

    pub fn founder_awe_id(&self) -> &AweId {
        &self.founder
    }

    pub fn installed_at_unix(&self) -> u64 {
        self.installed_at_unix
    }

    pub fn is_founder(&self, public_key: &[u8; 32]) -> bool {
        *public_key == self.founder_public_key
    }

    /// Backend permission check for founder-only operations. Appends an
    /// immutable audit entry when the caller is the founder and rejects every
    /// other caller.
    pub fn authorize(
        &mut self,
        actor_public_key: &[u8; 32],
        action: FounderAction,
        at_unix: u64,
        detail: &str,
    ) -> Result<u64, FounderError> {
        if !self.is_founder(actor_public_key) {
            return Err(FounderError::NotFounder);
        }
        if detail.len() > MAX_ACTION_DETAIL_CHARS {
            return Err(FounderError::DetailTooLong);
        }
        if self.log.len() >= MAX_FOUNDER_LOG {
            return Err(FounderError::LogFull);
        }
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(FounderError::SequenceExhausted)?;
        self.log.push(FounderAuditEntry {
            sequence,
            action,
            actor: self.founder.clone(),
            at_unix,
            detail: detail.to_string(),
        });
        Ok(sequence)
    }

    pub fn log(&self) -> &[FounderAuditEntry] {
        &self.log
    }

    pub fn log_len(&self) -> usize {
        self.log.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    #[test]
    fn founder_identity_cannot_be_replaced() {
        let founder = identity("founder");
        let attacker = identity("attacker");
        let mut authority = FounderAuthority::install(&founder, 0);
        assert!(authority.is_founder(&founder.public.public_key));
        assert!(!authority.is_founder(&attacker.public.public_key));
        assert_eq!(
            authority
                .authorize(
                    &attacker.public.public_key,
                    FounderAction::SuspendAccount,
                    1,
                    "attempt"
                )
                .unwrap_err(),
            FounderError::NotFounder
        );
        assert_eq!(authority.founder_awe_id(), &founder.public.awe_id);
        assert_eq!(authority.installed_at_unix(), 0);
        assert_eq!(authority.log_len(), 0);
    }

    #[test]
    fn founder_authorization_is_audited_in_order() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 10);
        let first = authority
            .authorize(
                &founder.public.public_key,
                FounderAction::PublishTask,
                20,
                "publish",
            )
            .unwrap();
        let second = authority
            .authorize(
                &founder.public.public_key,
                FounderAction::ReviewSubmission,
                30,
                "review",
            )
            .unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        let log = authority.log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].action, FounderAction::PublishTask);
        assert_eq!(log[0].actor, founder.public.awe_id);
        assert_eq!(log[0].at_unix, 20);
        assert_eq!(log[1].sequence, 2);
    }

    #[test]
    fn audit_log_is_bounded() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        for i in 0..MAX_FOUNDER_LOG as u64 {
            authority
                .authorize(
                    &founder.public.public_key,
                    FounderAction::ManageTreasury,
                    i,
                    "tick",
                )
                .unwrap();
        }
        assert_eq!(authority.log_len(), MAX_FOUNDER_LOG);
        assert_eq!(
            authority
                .authorize(
                    &founder.public.public_key,
                    FounderAction::ManageTreasury,
                    1,
                    "full"
                )
                .unwrap_err(),
            FounderError::LogFull
        );
    }

    #[test]
    fn action_detail_is_bounded() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let long = "x".repeat(MAX_ACTION_DETAIL_CHARS + 1);
        assert_eq!(
            authority
                .authorize(
                    &founder.public.public_key,
                    FounderAction::ManageTreasury,
                    1,
                    &long
                )
                .unwrap_err(),
            FounderError::DetailTooLong
        );
        assert_eq!(authority.log_len(), 0);
    }

    #[test]
    fn audit_sequence_exhaustion_does_not_append_or_wrap() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        authority.next_sequence = u64::MAX;

        assert_eq!(
            authority.authorize(
                &founder.public.public_key,
                FounderAction::ManageTreasury,
                1,
                "exhausted",
            ),
            Err(FounderError::SequenceExhausted)
        );
        assert_eq!(authority.log_len(), 0);
        assert_eq!(authority.next_sequence, u64::MAX);
    }
}
