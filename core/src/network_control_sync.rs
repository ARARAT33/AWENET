//! Bounded, authenticated transport messages for Founder-signed network snapshots.
//!
//! The transport authenticates the peer; the snapshot signature remains the
//! authority. Chunks are deliberately small so the runtime policy can bound
//! each message and the receiver can bound total reassembly memory.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use thiserror::Error;

pub const NETWORK_CONTROL_SYNC_STREAM: u32 = 204;
pub const NETWORK_CONTROL_SYNC_VERSION: u16 = 1;
pub const MAX_SYNC_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
pub const SYNC_CHUNK_BYTES: usize = 12 * 1024;
pub const MAX_SYNC_CHUNKS: usize =
    (MAX_SYNC_SNAPSHOT_BYTES + SYNC_CHUNK_BYTES - 1) / SYNC_CHUNK_BYTES;
pub const MAX_SYNC_MESSAGE_BYTES: usize = 64 * 1024;
const TRANSFER_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAckStatus {
    Applied,
    AlreadyCurrent,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotChunk {
    pub version: u16,
    pub sequence: u64,
    /// Digest of the exact inner snapshot string covered by the Founder signature.
    pub snapshot_digest: [u8; 32],
    /// Digest of the complete signed envelope transferred over this stream.
    pub transfer_digest: [u8; 32],
    pub total_bytes: u32,
    pub chunk_index: u16,
    pub chunk_count: u16,
    /// Hex keeps the JSON wire representation bounded and independent of
    /// serde's numeric-array representation for byte vectors.
    pub chunk_hex: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotAck {
    pub version: u16,
    pub sequence: u64,
    pub snapshot_digest: [u8; 32],
    pub status: SnapshotAckStatus,
}

/// Tracks only acknowledgements for transfers actually sent to an admitted peer.
/// A peer cannot pre-acknowledge a future snapshot or satisfy a different transfer.
#[derive(Default)]
pub struct SnapshotAckTracker {
    pending: BTreeMap<[u8; 32], (u64, [u8; 32])>,
    acknowledged: BTreeMap<[u8; 32], (u64, [u8; 32], SnapshotAckStatus)>,
}

impl SnapshotAckTracker {
    pub fn begin_transfer(&mut self, peer: [u8; 32], sequence: u64, digest: [u8; 32]) {
        self.acknowledged.remove(&peer);
        self.pending.insert(peer, (sequence, digest));
    }

    pub fn cancel_transfer(&mut self, peer: [u8; 32], sequence: u64, digest: [u8; 32]) {
        if self.pending.get(&peer) == Some(&(sequence, digest)) {
            self.pending.remove(&peer);
        }
    }

    pub fn accept_ack(&mut self, peer: [u8; 32], ack: &SnapshotAck) -> bool {
        if ack.version != NETWORK_CONTROL_SYNC_VERSION
            || self.pending.get(&peer) != Some(&(ack.sequence, ack.snapshot_digest))
        {
            return false;
        }
        self.pending.remove(&peer);
        self.acknowledged.insert(
            peer,
            (ack.sequence, ack.snapshot_digest, ack.status.clone()),
        );
        true
    }

    pub fn status_for(
        &self,
        peer: &[u8; 32],
        sequence: u64,
        digest: &[u8; 32],
    ) -> Option<SnapshotAckStatus> {
        self.acknowledged
            .get(peer)
            .and_then(|(seen_sequence, seen_digest, status)| {
                (*seen_sequence == sequence && seen_digest == digest).then(|| status.clone())
            })
    }

    pub fn forget_peer(&mut self, peer: &[u8; 32]) {
        self.pending.remove(peer);
        self.acknowledged.remove(peer);
    }

    pub fn retain_peers(&mut self, peers: &BTreeSet<[u8; 32]>) {
        self.pending.retain(|peer, _| peers.contains(peer));
        self.acknowledged.retain(|peer, _| peers.contains(peer));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum NetworkControlSyncMessage {
    SnapshotChunk(SnapshotChunk),
    SnapshotAck(SnapshotAck),
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum NetworkControlSyncError {
    #[error("network-control sync message exceeds the maximum size")]
    MessageTooLarge,
    #[error("network-control snapshot exceeds the sync transfer limit")]
    SnapshotTooLarge,
    #[error("network-control snapshot is empty")]
    EmptySnapshot,
    #[error("unsupported network-control sync message version")]
    InvalidVersion,
    #[error("malformed network-control sync message: {0}")]
    Malformed(String),
    #[error("network-control snapshot chunk metadata is invalid")]
    InvalidChunkMetadata,
    #[error("network-control snapshot sender is not the pinned Founder")]
    UnauthorizedSender,
    #[error("network-control snapshot transfer is older than the active partial transfer")]
    StaleTransfer,
    #[error("conflicting network-control snapshot transfers share one sequence")]
    SnapshotEquivocation,
    #[error("a repeated network-control snapshot chunk has different bytes")]
    ConflictingChunk,
    #[error("network-control snapshot transfer digest does not match its bytes")]
    TransferDigestMismatch,
    #[error("network-control snapshot metadata does not match the Founder-signed envelope")]
    SnapshotMetadataMismatch,
}

pub fn encode(message: &NetworkControlSyncMessage) -> Result<Vec<u8>, NetworkControlSyncError> {
    match message {
        NetworkControlSyncMessage::SnapshotChunk(chunk)
            if chunk.version != NETWORK_CONTROL_SYNC_VERSION =>
        {
            return Err(NetworkControlSyncError::InvalidVersion);
        }
        NetworkControlSyncMessage::SnapshotAck(ack)
            if ack.version != NETWORK_CONTROL_SYNC_VERSION =>
        {
            return Err(NetworkControlSyncError::InvalidVersion);
        }
        _ => {}
    }

    let bytes = serde_json::to_vec(message)
        .map_err(|error| NetworkControlSyncError::Malformed(error.to_string()))?;
    if bytes.len() > MAX_SYNC_MESSAGE_BYTES {
        return Err(NetworkControlSyncError::MessageTooLarge);
    }
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<NetworkControlSyncMessage, NetworkControlSyncError> {
    if bytes.len() > MAX_SYNC_MESSAGE_BYTES {
        return Err(NetworkControlSyncError::MessageTooLarge);
    }
    let message: NetworkControlSyncMessage = serde_json::from_slice(bytes)
        .map_err(|error| NetworkControlSyncError::Malformed(error.to_string()))?;
    match &message {
        NetworkControlSyncMessage::SnapshotChunk(chunk)
            if chunk.version != NETWORK_CONTROL_SYNC_VERSION =>
        {
            Err(NetworkControlSyncError::InvalidVersion)
        }
        NetworkControlSyncMessage::SnapshotAck(ack)
            if ack.version != NETWORK_CONTROL_SYNC_VERSION =>
        {
            Err(NetworkControlSyncError::InvalidVersion)
        }
        _ => Ok(message),
    }
}

/// Splits one already-signed Founder envelope into bounded transport messages.
/// The snapshot digest is the value retained by the local anti-rollback anchors.
pub fn chunk_snapshot(
    bytes: &[u8],
    sequence: u64,
    snapshot_digest: [u8; 32],
) -> Result<Vec<NetworkControlSyncMessage>, NetworkControlSyncError> {
    if bytes.is_empty() {
        return Err(NetworkControlSyncError::EmptySnapshot);
    }
    if bytes.len() > MAX_SYNC_SNAPSHOT_BYTES {
        return Err(NetworkControlSyncError::SnapshotTooLarge);
    }

    let chunk_count = (bytes.len() + SYNC_CHUNK_BYTES - 1) / SYNC_CHUNK_BYTES;
    if chunk_count == 0 || chunk_count > MAX_SYNC_CHUNKS {
        return Err(NetworkControlSyncError::InvalidChunkMetadata);
    }
    let transfer_digest = *blake3::hash(bytes).as_bytes();
    let total_bytes =
        u32::try_from(bytes.len()).map_err(|_| NetworkControlSyncError::SnapshotTooLarge)?;
    let mut messages = Vec::with_capacity(chunk_count);

    for (index, chunk) in bytes.chunks(SYNC_CHUNK_BYTES).enumerate() {
        let message = NetworkControlSyncMessage::SnapshotChunk(SnapshotChunk {
            version: NETWORK_CONTROL_SYNC_VERSION,
            sequence,
            snapshot_digest,
            transfer_digest,
            total_bytes,
            chunk_index: index as u16,
            chunk_count: chunk_count as u16,
            chunk_hex: hex::encode(chunk),
        });
        encode(&message)?;
        messages.push(message);
    }
    Ok(messages)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedSnapshot {
    pub sequence: u64,
    pub snapshot_digest: [u8; 32],
    pub bytes: Vec<u8>,
}

/// Verify that transfer metadata is bound to the exact Founder-signed envelope.
/// The transport digest protects reassembly; this check binds sequence and
/// anti-rollback digest to the signed network-control payload.
pub fn verify_snapshot_metadata(
    bytes: &[u8],
    sequence: u64,
    snapshot_digest: [u8; 32],
    trusted_founder_public_key: &[u8; 32],
) -> Result<(), NetworkControlSyncError> {
    let verified =
        crate::network_control::load_verified_snapshot_bytes(bytes, trusted_founder_public_key)
            .map_err(|error| NetworkControlSyncError::Malformed(error.to_string()))?;
    if verified.sequence() != sequence || verified.digest() != snapshot_digest {
        return Err(NetworkControlSyncError::SnapshotMetadataMismatch);
    }
    Ok(())
}

struct PendingSnapshot {
    sequence: u64,
    snapshot_digest: [u8; 32],
    transfer_digest: [u8; 32],
    total_bytes: usize,
    chunk_count: usize,
    chunks: Vec<Option<Vec<u8>>>,
    received_bytes: usize,
    last_updated: Instant,
}

/// Reassembles at most one transfer from the one Founder pinned by this Node.
/// The 60-second inactivity expiry and 16 MiB hard cap bound memory use.
#[derive(Default)]
pub struct SnapshotAssembler {
    pending: Option<PendingSnapshot>,
}

impl SnapshotAssembler {
    pub fn pending_bytes(&self) -> usize {
        self.pending
            .as_ref()
            .map(|pending| pending.received_bytes)
            .unwrap_or(0)
    }

    pub fn push_chunk(
        &mut self,
        sender: &[u8; 32],
        expected_founder: &[u8; 32],
        chunk: &SnapshotChunk,
        now: Instant,
    ) -> Result<Option<CompletedSnapshot>, NetworkControlSyncError> {
        if sender != expected_founder {
            return Err(NetworkControlSyncError::UnauthorizedSender);
        }
        if chunk.version != NETWORK_CONTROL_SYNC_VERSION {
            return Err(NetworkControlSyncError::InvalidVersion);
        }

        let total_bytes = chunk.total_bytes as usize;
        let chunk_count = chunk.chunk_count as usize;
        let expected_count = if total_bytes == 0 {
            0
        } else {
            (total_bytes + SYNC_CHUNK_BYTES - 1) / SYNC_CHUNK_BYTES
        };
        if total_bytes == 0
            || total_bytes > MAX_SYNC_SNAPSHOT_BYTES
            || chunk_count == 0
            || chunk_count > MAX_SYNC_CHUNKS
            || chunk_count != expected_count
            || chunk.chunk_index as usize >= chunk_count
            || chunk.chunk_hex.len() > SYNC_CHUNK_BYTES * 2
        {
            return Err(NetworkControlSyncError::InvalidChunkMetadata);
        }

        let index = chunk.chunk_index as usize;
        let expected_chunk_bytes = if index + 1 == chunk_count {
            total_bytes - index * SYNC_CHUNK_BYTES
        } else {
            SYNC_CHUNK_BYTES
        };
        let decoded = hex::decode(&chunk.chunk_hex)
            .map_err(|error| NetworkControlSyncError::Malformed(error.to_string()))?;
        if decoded.len() != expected_chunk_bytes {
            return Err(NetworkControlSyncError::InvalidChunkMetadata);
        }

        if self.pending.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.last_updated) > TRANSFER_TTL
        }) {
            self.pending = None;
        }

        let same_transfer = self.pending.as_ref().is_some_and(|pending| {
            pending.sequence == chunk.sequence
                && pending.snapshot_digest == chunk.snapshot_digest
                && pending.transfer_digest == chunk.transfer_digest
                && pending.total_bytes == total_bytes
                && pending.chunk_count == chunk_count
        });

        if !same_transfer {
            if let Some(pending) = self.pending.as_ref() {
                if chunk.sequence < pending.sequence {
                    return Err(NetworkControlSyncError::StaleTransfer);
                }
                if chunk.sequence == pending.sequence {
                    return Err(NetworkControlSyncError::SnapshotEquivocation);
                }
            }
            self.pending = Some(PendingSnapshot {
                sequence: chunk.sequence,
                snapshot_digest: chunk.snapshot_digest,
                transfer_digest: chunk.transfer_digest,
                total_bytes,
                chunk_count,
                chunks: vec![None; chunk_count],
                received_bytes: 0,
                last_updated: now,
            });
        }

        let pending = self
            .pending
            .as_mut()
            .ok_or(NetworkControlSyncError::InvalidChunkMetadata)?;
        pending.last_updated = now;

        if let Some(existing) = pending.chunks[index].as_ref() {
            if existing != &decoded {
                self.pending = None;
                return Err(NetworkControlSyncError::ConflictingChunk);
            }
            return Ok(None);
        }

        pending.received_bytes = pending
            .received_bytes
            .checked_add(decoded.len())
            .ok_or(NetworkControlSyncError::InvalidChunkMetadata)?;
        if pending.received_bytes > pending.total_bytes {
            self.pending = None;
            return Err(NetworkControlSyncError::InvalidChunkMetadata);
        }
        pending.chunks[index] = Some(decoded);

        if pending.chunks.iter().any(Option::is_none) {
            return Ok(None);
        }

        let pending = self
            .pending
            .take()
            .ok_or(NetworkControlSyncError::InvalidChunkMetadata)?;
        let mut bytes = Vec::with_capacity(pending.total_bytes);
        for part in pending.chunks {
            let part = part.ok_or(NetworkControlSyncError::InvalidChunkMetadata)?;
            bytes.extend_from_slice(&part);
        }
        if bytes.len() != pending.total_bytes
            || *blake3::hash(&bytes).as_bytes() != pending.transfer_digest
        {
            return Err(NetworkControlSyncError::TransferDigestMismatch);
        }

        Ok(Some(CompletedSnapshot {
            sequence: pending.sequence,
            snapshot_digest: pending.snapshot_digest,
            bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(bytes: &[u8]) -> Vec<SnapshotChunk> {
        chunk_snapshot(bytes, 7, [9; 32])
            .unwrap()
            .into_iter()
            .map(|message| match message {
                NetworkControlSyncMessage::SnapshotChunk(chunk) => chunk,
                NetworkControlSyncMessage::SnapshotAck(_) => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn assembles_out_of_order_chunks_and_ignores_identical_duplicates() {
        let bytes = vec![0x5a; SYNC_CHUNK_BYTES + 17];
        let chunks = chunks(&bytes);
        let sender = [1; 32];
        let mut assembler = SnapshotAssembler::default();
        assert!(assembler
            .push_chunk(&sender, &sender, &chunks[1], Instant::now())
            .unwrap()
            .is_none());
        assert!(assembler
            .push_chunk(&sender, &sender, &chunks[1], Instant::now())
            .unwrap()
            .is_none());
        let complete = assembler
            .push_chunk(&sender, &sender, &chunks[0], Instant::now())
            .unwrap()
            .unwrap();
        assert_eq!(complete.sequence, 7);
        assert_eq!(complete.snapshot_digest, [9; 32]);
        assert_eq!(complete.bytes, bytes);
        assert_eq!(assembler.pending_bytes(), 0);
    }

    #[test]
    fn rejects_non_founder_before_allocating_reassembly_state() {
        let chunks = chunks(b"small signed envelope");
        let mut assembler = SnapshotAssembler::default();
        assert_eq!(
            assembler.push_chunk(&[2; 32], &[1; 32], &chunks[0], Instant::now()),
            Err(NetworkControlSyncError::UnauthorizedSender)
        );
        assert_eq!(assembler.pending_bytes(), 0);
    }

    #[test]
    fn rejects_conflicting_duplicate_chunk() {
        let bytes = vec![0x33; SYNC_CHUNK_BYTES + 1];
        let mut chunks = chunks(&bytes);
        let sender = [1; 32];
        let mut assembler = SnapshotAssembler::default();
        assembler
            .push_chunk(&sender, &sender, &chunks[0], Instant::now())
            .unwrap();
        chunks[0].chunk_hex.replace_range(0..2, "ff");
        assert_eq!(
            assembler.push_chunk(&sender, &sender, &chunks[0], Instant::now()),
            Err(NetworkControlSyncError::ConflictingChunk)
        );
        assert_eq!(assembler.pending_bytes(), 0);
    }

    #[test]
    fn rejects_bad_transfer_digest_after_all_chunks_arrive() {
        let bytes = vec![0x44; SYNC_CHUNK_BYTES + 1];
        let mut chunks = chunks(&bytes);
        chunks[0].transfer_digest[0] ^= 1;
        chunks[1].transfer_digest[0] ^= 1;
        let sender = [1; 32];
        let mut assembler = SnapshotAssembler::default();
        assembler
            .push_chunk(&sender, &sender, &chunks[0], Instant::now())
            .unwrap();
        assert_eq!(
            assembler.push_chunk(&sender, &sender, &chunks[1], Instant::now()),
            Err(NetworkControlSyncError::TransferDigestMismatch)
        );
    }

    #[test]
    fn rejects_oversized_snapshot_before_chunking() {
        let bytes = vec![0; MAX_SYNC_SNAPSHOT_BYTES + 1];
        assert_eq!(
            chunk_snapshot(&bytes, 1, [0; 32]),
            Err(NetworkControlSyncError::SnapshotTooLarge)
        );
    }

    #[test]
    fn wire_messages_round_trip_and_respect_the_message_limit() {
        let message = NetworkControlSyncMessage::SnapshotAck(SnapshotAck {
            version: NETWORK_CONTROL_SYNC_VERSION,
            sequence: 8,
            snapshot_digest: [7; 32],
            status: SnapshotAckStatus::Applied,
        });
        let encoded = encode(&message).unwrap();
        assert_eq!(decode(&encoded).unwrap(), message);
        assert_eq!(
            decode(&vec![0; MAX_SYNC_MESSAGE_BYTES + 1]),
            Err(NetworkControlSyncError::MessageTooLarge)
        );
    }

    #[test]
    fn acknowledgements_must_match_a_pending_transfer() {
        let peer = [4; 32];
        let mut tracker = SnapshotAckTracker::default();
        let ack = SnapshotAck {
            version: NETWORK_CONTROL_SYNC_VERSION,
            sequence: 7,
            snapshot_digest: [9; 32],
            status: SnapshotAckStatus::Applied,
        };

        assert!(!tracker.accept_ack(peer, &ack));
        tracker.begin_transfer(peer, 7, [8; 32]);
        assert!(!tracker.accept_ack(peer, &ack));
        tracker.begin_transfer(peer, 7, [9; 32]);
        assert!(tracker.accept_ack(peer, &ack));
        assert_eq!(
            tracker.status_for(&peer, 7, &[9; 32]),
            Some(SnapshotAckStatus::Applied)
        );
        assert!(!tracker.accept_ack(peer, &ack));
    }

    #[test]
    fn rejected_ack_can_be_retried_and_old_peer_state_is_pruned() {
        let peer = [5; 32];
        let mut tracker = SnapshotAckTracker::default();
        tracker.begin_transfer(peer, 3, [6; 32]);
        let rejected = SnapshotAck {
            version: NETWORK_CONTROL_SYNC_VERSION,
            sequence: 3,
            snapshot_digest: [6; 32],
            status: SnapshotAckStatus::Rejected,
        };
        assert!(tracker.accept_ack(peer, &rejected));
        assert_eq!(
            tracker.status_for(&peer, 3, &[6; 32]),
            Some(SnapshotAckStatus::Rejected)
        );

        tracker.begin_transfer(peer, 3, [6; 32]);
        assert_eq!(tracker.status_for(&peer, 3, &[6; 32]), None);
        let applied = SnapshotAck {
            status: SnapshotAckStatus::Applied,
            ..rejected
        };
        assert!(tracker.accept_ack(peer, &applied));

        tracker.retain_peers(&BTreeSet::new());
        assert_eq!(tracker.status_for(&peer, 3, &[6; 32]), None);
    }

    #[test]
    fn snapshot_metadata_must_match_the_signed_envelope() {
        use crate::{
            identity::{Identity, Username},
            network_control::{self, NetworkControlPlane},
        };
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };

        let founder = Identity::generate(Username::new("sync-metadata-founder").unwrap());
        let control = NetworkControlPlane::new(&founder, 100);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "awenet-sync-metadata-{}-{nonce}.json",
            std::process::id()
        ));
        network_control::save(&path, &control, &founder).unwrap();
        let bytes = fs::read(&path).unwrap();
        let verified =
            network_control::load_verified_snapshot_bytes(&bytes, &founder.public.public_key)
                .unwrap();

        assert!(verify_snapshot_metadata(
            &bytes,
            verified.sequence(),
            verified.digest(),
            &founder.public.public_key,
        )
        .is_ok());
        assert_eq!(
            verify_snapshot_metadata(
                &bytes,
                verified.sequence().saturating_add(1),
                verified.digest(),
                &founder.public.public_key,
            ),
            Err(NetworkControlSyncError::SnapshotMetadataMismatch)
        );
        let mut wrong_digest = verified.digest();
        wrong_digest[0] ^= 1;
        assert_eq!(
            verify_snapshot_metadata(
                &bytes,
                verified.sequence(),
                wrong_digest,
                &founder.public.public_key,
            ),
            Err(NetworkControlSyncError::SnapshotMetadataMismatch)
        );

        let _ = fs::remove_file(path);
    }
}
