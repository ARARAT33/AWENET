//! AWEp2P platform-independent core and real peer-to-peer transport.
//! Platform clients share identity, security, networking, namespace, storage,
//! hosting, privacy-first messaging, diagnostics, governance, and application store primitives.

pub mod calls;
pub mod canonical;
pub mod crypto;
pub mod data_plane;
pub mod diagnostics;
pub mod governance;
pub mod host;
pub mod host_directory;
pub mod identity;
pub mod lan_mesh;
pub mod limits;
pub mod messenger;
pub mod namespace;
pub mod network;
pub mod node;
pub mod permissions;
pub mod protocol;
pub mod recovery;
pub mod registry;
pub mod replay;
pub mod security;
pub mod replication;
pub mod reputation;
pub mod routing;
pub mod sandbox;
pub mod storage;
pub mod store;

pub mod network_topology;
