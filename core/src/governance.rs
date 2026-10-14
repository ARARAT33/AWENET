//! Decentralized TLD Governance, capability-scoped delegation, and administrative policy rules.

use crate::registry::{RegistryRecord, TldKind};
use serde::{Deserialize, Serialize};

pub const DEFAULT_OTLD_MAX_DOMAINS: u32 = 10;
pub const PROTECTED_ADMIN_NAMESPACE: &str = ".awea";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovernanceCapability {
    pub issuer: Vec<u8>,
    pub holder: Vec<u8>,
    pub scope: String,
    pub kind: TldKind,
    pub max_domains: Option<u32>,
    pub expires_at: u64,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GovernanceAuditRecord {
    pub timestamp: u64,
    pub actor_public_key: Vec<u8>,
    pub action: String,
    pub target: String,
    pub details: String,
}

pub struct GovernanceEngine;

impl GovernanceEngine {
    pub fn validate_domain_registration(
        tld_record: &RegistryRecord,
        user_domain_count: u32,
    ) -> Result<(), &'static str> {
        let kind = tld_record.kind.as_ref().ok_or("missing TLD kind")?;
        match kind {
            TldKind::Otld => {
                if user_domain_count >= DEFAULT_OTLD_MAX_DOMAINS {
                    return Err("OTLD domain quota exceeded (max 10 domains)");
                }
            }
            TldKind::ThreeOtld => {
                // Open creation according to policy
            }
            TldKind::Ctld => {
                // Requires community creator authorization
            }
            TldKind::Atld | TldKind::Vtld | TldKind::Oatld | TldKind::Octld => {
                // Managed TLDs
            }
            TldKind::Autl => {
                // Specific user assigned namespace
            }
        }
        Ok(())
    }

    pub fn is_protected_namespace(name: &str) -> bool {
        name == "awea" || name.ends_with(PROTECTED_ADMIN_NAMESPACE)
    }

    /// Canonical payload signed by the capability issuer. The signature itself is
    /// deliberately excluded from the payload.
    pub fn capability_signing_bytes(cap: &GovernanceCapability) -> Result<Vec<u8>, &'static str> {
        serde_json::to_vec(&(
            cap.issuer.as_slice(),
            cap.holder.as_slice(),
            cap.scope.as_str(),
            &cap.kind,
            cap.max_domains,
            cap.expires_at,
        ))
        .map_err(|_| "governance capability serialization failed")
    }

    /// Verify issuer signature, actor/holder binding, expiry and path-segment scope.
    /// A capability is never authorization merely because its fields look plausible.
    pub fn authorize_action(
        cap: &GovernanceCapability,
        actor_public_key: &[u8],
        target: &str,
        now: u64,
    ) -> Result<(), &'static str> {
        use crate::identity::Identity;

        if cap.expires_at > 0 && now >= cap.expires_at {
            return Err("governance capability expired");
        }
        if cap.issuer.len() != 32 || cap.holder.len() != 32 || cap.signature.len() != 64 {
            return Err("invalid governance capability key or signature length");
        }
        if cap.holder.as_slice() != actor_public_key {
            return Err("governance capability holder mismatch");
        }
        let mut issuer = [0u8; 32];
        issuer.copy_from_slice(&cap.issuer);
        let payload = Self::capability_signing_bytes(cap)?;
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&cap.signature);
        if !Identity::verify(&issuer, &payload, &signature) {
            return Err("invalid governance capability signature");
        }
        let in_scope = target == cap.scope
            || target
                .strip_prefix(&cap.scope)
                .map(|suffix| {
                    suffix.starts_with('/') || suffix.starts_with('.') || suffix.starts_with(':')
                })
                .unwrap_or(false);
        if !in_scope {
            return Err("action out of capability scope");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryStatus;

    #[test]
    fn otld_quota_enforced() {
        let record = RegistryRecord {
            object_type: crate::registry::RegistryObjectType::Tld,
            kind: Some(TldKind::Otld),
            name: "user".into(),
            owner_public_key: vec![0u8; 32],
            parent: None,
            status: RegistryStatus::Active,
            sequence: 1,
            content_hash: [0u8; 32],
            signature: vec![],
        };

        assert!(GovernanceEngine::validate_domain_registration(&record, 9).is_ok());
        assert!(GovernanceEngine::validate_domain_registration(&record, 10).is_err());
    }

    #[test]
    fn capability_requires_issuer_signature_and_correct_holder() {
        use crate::identity::{Identity, Username};

        let issuer = Identity::generate(Username::new("issuer").unwrap());
        let holder = Identity::generate(Username::new("holder").unwrap());
        let mut cap = GovernanceCapability {
            issuer: issuer.public.public_key.to_vec(),
            holder: holder.public.public_key.to_vec(),
            scope: "network/dc-1".into(),
            kind: TldKind::Ctld,
            max_domains: Some(2),
            expires_at: 500,
            signature: Vec::new(),
        };
        cap.signature = issuer
            .sign(&GovernanceEngine::capability_signing_bytes(&cap).unwrap())
            .to_vec();
        assert!(GovernanceEngine::authorize_action(
            &cap,
            &holder.public.public_key,
            "network/dc-1/node-2",
            100
        )
        .is_ok());
        assert!(GovernanceEngine::authorize_action(
            &cap,
            &issuer.public.public_key,
            "network/dc-1/node-2",
            100
        )
        .is_err());
        assert!(GovernanceEngine::authorize_action(
            &cap,
            &holder.public.public_key,
            "network/dc-10",
            100
        )
        .is_err());
        cap.signature[0] ^= 1;
        assert!(GovernanceEngine::authorize_action(
            &cap,
            &holder.public.public_key,
            "network/dc-1",
            100
        )
        .is_err());
    }

    #[test]
    fn awea_protected() {
        assert!(GovernanceEngine::is_protected_namespace("sys.awea"));
        assert!(!GovernanceEngine::is_protected_namespace("example.com"));
    }
}
