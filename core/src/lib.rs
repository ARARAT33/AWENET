pub mod serde_bytes_64 {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(value: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::invalid_length(bytes.len(), &"64-byte signature"));
        }
        let mut value = [0u8; 64];
        value.copy_from_slice(&bytes);
        Ok(value)
    }
}

pub mod calls;
pub mod canonical;
pub mod crypto;
pub mod access;
pub mod awenet;
pub mod data_plane;
pub mod defense;
pub mod diagnostics;
pub mod discovery;
pub mod federation;
pub mod governance;
pub mod host;
pub mod host_directory;
pub mod identity;
pub mod lan_mesh;
pub mod limits;
pub mod messenger;
pub mod messenger_runtime;
pub mod namespace;
pub mod network;
pub mod network_topology;
pub mod onecoin;
pub mod onecoin_store;
pub mod onecoin_payment;
pub mod onecoin_governance;
pub mod onecoin_consensus;
pub mod onecoin_consensus_runtime;
pub mod node;
pub mod permissions;
pub mod policy;
pub mod product;
pub mod protocol;
pub mod readiness;
pub mod recovery;
pub mod registry;
pub mod repair;
pub mod replay;
pub mod replication;
pub mod reputation;
pub mod routing;
pub mod sandbox;
pub mod security;
pub mod storage;
pub mod store;
pub mod supervisor;
