//! Capability-based access for Secret Node / Secret Site surfaces.
//!
//! Open resources are addressable by AWE-ID/site ID. Secret resources require
//! possession of an access map. The network never needs to learn the map value:
//! only a one-way commitment is published with the resource metadata.

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessMode {
    Open,
    Secret,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessMap {
    pub version: u16,
    pub resource_id: [u8; 32],
    pub mode: AccessMode,
    /// Capability itself. Treat this object like a private key.
    pub capability: [u8; 32],
    /// Public commitment published with the resource.
    pub commitment: [u8; 32],
}

impl AccessMap {
    pub const VERSION: u16 = 1;

    pub fn for_node(node_id: [u8; 32], mode: AccessMode) -> Self {
        Self::generate(node_id, mode)
    }

    pub fn for_site(site_id: [u8; 32], mode: AccessMode) -> Self {
        Self::generate(site_id, mode)
    }

    pub fn generate(resource_id: [u8; 32], mode: AccessMode) -> Self {
        let mut capability = [0u8; 32];
        OsRng.fill_bytes(&mut capability);
        let commitment = Self::commitment(&resource_id, &capability);
        Self {
            version: Self::VERSION,
            resource_id,
            mode,
            capability,
            commitment,
        }
    }

    pub fn commitment(resource_id: &[u8; 32], capability: &[u8; 32]) -> [u8; 32] {
        *blake3::hash(
            &[
                b"AWE/ACCESS-MAP/v1".as_slice(),
                resource_id.as_slice(),
                capability.as_slice(),
            ]
            .concat(),
        )
        .as_bytes()
    }

    pub fn verify(&self, resource_id: &[u8; 32], published_commitment: &[u8; 32]) -> bool {
        self.version == Self::VERSION
            && self.resource_id == *resource_id
            && self.mode == AccessMode::Secret
            && &Self::commitment(resource_id, &self.capability) == published_commitment
            && self.commitment == *published_commitment
    }

    pub fn allows_open_id(mode: AccessMode, supplied_map: Option<&Self>) -> bool {
        match mode {
            AccessMode::Open => true,
            AccessMode::Secret => supplied_map.is_some(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessDescriptor {
    pub mode: AccessMode,
    pub commitment: Option<[u8; 32]>,
}

impl AccessDescriptor {
    pub fn open() -> Self {
        Self {
            mode: AccessMode::Open,
            commitment: None,
        }
    }

    pub fn secret(map: &AccessMap) -> Self {
        Self {
            mode: AccessMode::Secret,
            commitment: Some(map.commitment),
        }
    }

    pub fn authorize(&self, map: Option<&AccessMap>, resource_id: &[u8; 32]) -> bool {
        match self.mode {
            AccessMode::Open => true,
            AccessMode::Secret => self
                .commitment
                .as_ref()
                .zip(map)
                .map(|(commitment, map)| map.verify(resource_id, commitment))
                .unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_is_available_by_id() {
        let id = [7u8; 32];
        let d = AccessDescriptor::open();
        assert!(d.authorize(None, &id));
    }

    #[test]
    fn secret_requires_the_access_map() {
        let id = [8u8; 32];
        let map = AccessMap::generate(id, AccessMode::Secret);
        let d = AccessDescriptor::secret(&map);
        assert!(!d.authorize(None, &id));
        assert!(d.authorize(Some(&map), &id));
        let other = AccessMap::generate(id, AccessMode::Secret);
        assert!(!d.authorize(Some(&other), &id));
    }
}
