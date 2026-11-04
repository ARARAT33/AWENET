use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand_core::{OsRng, RngCore};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8] = b"AWE-DRIVE-V1";
const NONCE_LEN: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub index: u32,
    pub hash: [u8; 32],
    pub size: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoragePolicy {
    pub data_shards: usize,
    pub parity_shards: usize,
    pub max_chunk_size: usize,
    pub replica_count: usize,
}

impl StoragePolicy {
    pub fn hyper_sovereign() -> Self {
        Self {
            data_shards: 450,
            parity_shards: 550,
            max_chunk_size: 4 * 1024 * 1024,
            replica_count: 3,
        }
    }

    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Scale shard count to file size while preserving the 1000-shard ceiling
    /// used by the full-size network policy. Small files stay cheap instead of
    /// paying the CPU/network cost of thousands of mostly empty shards.
    pub fn for_file_size(bytes: usize) -> Self {
        let total = if bytes == 0 {
            12
        } else {
            let chunk_groups = bytes.saturating_add(4 * 1024 * 1024 - 1) / (4 * 1024 * 1024);
            (chunk_groups.saturating_mul(12)).clamp(12, 1000)
        };
        Self::custom_scaled(total)
    }

    pub fn custom_scaled(total_shards: usize) -> Self {
        let total = total_shards.clamp(12, 100_000_000);
        let data = (total as f64 * 0.45) as usize;
        let parity = total - data;
        Self {
            data_shards: data,
            parity_shards: parity,
            max_chunk_size: 4 * 1024 * 1024,
            replica_count: 3,
        }
    }
}

impl Default for StoragePolicy {
    fn default() -> Self {
        Self::custom_scaled(12)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateManifest {
    pub file_id: [u8; 32],
    pub original_size: u64,
    pub chunks: Vec<ChunkRef>,
    pub policy: StoragePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsMap {
    pub file_id: [u8; 32],
    pub site_id: Option<String>,
    pub filename: String,
    pub total_shards: usize,
    pub data_shards: usize,
    pub parity_shards: usize,
    pub shard_nodes: Vec<Vec<String>>,
}

impl AsMap {
    /// Assign all 1000 shards to deterministic, distinct three-node replica sets.
    /// The node list must come from real discovered peers; this method never invents IDs.
    pub fn assign_all_replicas(&mut self, node_ids: &[String]) -> Result<(), String> {
        if self.total_shards != crate::replication::REQUIRED_SHARDS {
            return Err("AWEP2P storage policy must contain exactly 1000 shards".into());
        }
        if node_ids.len() < crate::replication::REQUIRED_REPLICAS {
            return Err("at least three distinct nodes are required".into());
        }
        for index in 0..self.total_shards {
            let replicas = crate::replication::select_replicas(&self.file_id, index, node_ids);
            if replicas.len() != crate::replication::REQUIRED_REPLICAS {
                return Err(format!("unable to place shard {index} on three nodes"));
            }
            self.assign_replicas(index, replicas)?;
        }
        Ok(())
    }

    pub fn validate_complete_placement(&self) -> Result<(), String> {
        if self.total_shards != crate::replication::REQUIRED_SHARDS
            || self.shard_nodes.len() != crate::replication::REQUIRED_SHARDS
        {
            return Err("manifest does not contain exactly 1000 shard entries".into());
        }
        for (index, replicas) in self.shard_nodes.iter().enumerate() {
            if replicas.len() != crate::replication::REQUIRED_REPLICAS {
                return Err(format!(
                    "shard {index} does not have exactly three replicas"
                ));
            }
            let mut unique = replicas.clone();
            unique.sort();
            unique.dedup();
            if unique.len() != crate::replication::REQUIRED_REPLICAS
                || unique.iter().any(|id| id.is_empty())
            {
                return Err(format!("shard {index} has invalid replica IDs"));
            }
        }
        Ok(())
    }
    pub fn new(file_id: [u8; 32], filename: String, site_id: Option<String>) -> Self {
        let policy = StoragePolicy::hyper_sovereign();
        Self {
            file_id,
            site_id,
            filename,
            total_shards: policy.data_shards + policy.parity_shards,
            data_shards: policy.data_shards,
            parity_shards: policy.parity_shards,
            // Placement is intentionally empty until actual peer IDs are selected.
            // Fabricated node IDs would make a local manifest look distributed without
            // corresponding real replicas on the network.
            shard_nodes: vec![Vec::new(); policy.data_shards + policy.parity_shards],
        }
    }

    pub fn assign_replicas(
        &mut self,
        shard_index: usize,
        node_ids: Vec<String>,
    ) -> Result<(), String> {
        if shard_index >= self.total_shards {
            return Err("shard index out of range".into());
        }
        if node_ids.len() != 3 {
            return Err("exactly three replica nodes are required".into());
        }
        let mut unique = node_ids;
        unique.sort();
        unique.dedup();
        if unique.len() != 3 || unique.iter().any(|id| id.is_empty()) {
            return Err("replica node IDs must be three distinct non-empty IDs".into());
        }
        self.shard_nodes[shard_index] = unique;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

pub const DAILY_UPLOAD_LIMIT_BYTES: u64 = 50 * 1024 * 1024 * 1024; // 50 GB

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyQuotaTracker {
    pub uploaded_today: u64,
    pub last_reset_timestamp: u64,
}

impl Default for DailyQuotaTracker {
    fn default() -> Self {
        Self {
            uploaded_today: 0,
            last_reset_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

impl DailyQuotaTracker {
    pub fn check_and_add(&mut self, bytes: u64) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if now.saturating_sub(self.last_reset_timestamp) >= 86400 {
            self.uploaded_today = 0;
            self.last_reset_timestamp = now;
        }
        if self.uploaded_today.saturating_add(bytes) > DAILY_UPLOAD_LIMIT_BYTES {
            return Err(format!(
                "AWEDrive 50GB/day limit exceeded. Uploaded today: {} MB, requested: {} MB",
                self.uploaded_today / (1024 * 1024),
                bytes / (1024 * 1024)
            ));
        }
        self.uploaded_today += bytes;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicManifest {
    pub file_id: [u8; 32],
    pub original_size: u64,
    pub chunks: Vec<ChunkRef>,
    pub policy: StoragePolicy,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeStorageStats {
    pub capacity: u64,
    pub used: u64,
    pub objects: u64,
    pub healthy_replicas: u64,
    pub repaired_chunks: u64,
}

pub fn content_id(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn choose_chunk_size(file_size: u64, max_chunk_size: usize) -> usize {
    let target = if file_size < 16 * 1024 * 1024 {
        256 * 1024
    } else if file_size < 1024 * 1024 * 1024 {
        1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    target.min(max_chunk_size.max(64 * 1024))
}

pub fn encrypt_file(data: &[u8], key_bytes: &[u8; 32]) -> io::Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), data)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "encryption failed"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_file(blob: &[u8], key_bytes: &[u8; 32]) -> io::Result<Vec<u8>> {
    if blob.len() < MAGIC.len() + NONCE_LEN || &blob[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid AWE Drive object",
        ));
    }
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key_bytes));
    cipher
        .decrypt(
            Nonce::from_slice(&blob[MAGIC.len()..MAGIC.len() + NONCE_LEN]),
            &blob[MAGIC.len() + NONCE_LEN..],
        )
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "authentication failed"))
}

pub fn encode_shards(data: &[u8], policy: &StoragePolicy) -> io::Result<Vec<Vec<u8>>> {
    if policy.data_shards == 0 || policy.parity_shards == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid erasure policy",
        ));
    }
    if data.is_empty() {
        return Ok(vec![
            vec![0u8; 1];
            policy.data_shards + policy.parity_shards
        ]);
    }
    let shard_len = data.len().div_ceil(policy.data_shards);
    let mut shards = vec![vec![0u8; shard_len]; policy.data_shards + policy.parity_shards];
    for (i, byte) in data.iter().enumerate() {
        shards[i / shard_len][i % shard_len] = *byte;
    }
    ReedSolomon::new(policy.data_shards, policy.parity_shards)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid Reed-Solomon policy"))?
        .encode(&mut shards)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "erasure encoding failed"))?;
    Ok(shards)
}

pub fn recover_shards(
    shards: &mut [Option<Vec<u8>>],
    policy: &StoragePolicy,
) -> io::Result<Vec<u8>> {
    let rs = ReedSolomon::new(policy.data_shards, policy.parity_shards)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid Reed-Solomon policy"))?;
    rs.reconstruct(shards)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "insufficient shards"))?;
    let mut out = Vec::new();
    for shard in shards.iter().take(policy.data_shards) {
        out.extend_from_slice(
            shard
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing data shard"))?,
        );
    }
    Ok(out)
}

pub struct LocalNodeStore {
    root: PathBuf,
    quota: u64,
}
impl LocalNodeStore {
    pub fn open(root: impl AsRef<Path>, quota: u64) -> io::Result<Self> {
        fs::create_dir_all(root.as_ref().join("objects"))?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            quota,
        })
    }
    fn path(&self, id: &[u8; 32]) -> PathBuf {
        self.root.join("objects").join(hex::encode(id))
    }
    pub fn put(&self, data: &[u8]) -> io::Result<[u8; 32]> {
        let id = content_id(data);
        let p = self.path(&id);
        let existing_len = match fs::metadata(&p) {
            Ok(metadata) => match self.get(&id) {
                Ok(_) => return Ok(id),
                Err(error) if error.kind() == io::ErrorKind::InvalidData => metadata.len(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
                Err(error) => return Err(error),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        let used = self.stats()?.used.saturating_sub(existing_len);
        if used.saturating_add(data.len() as u64) > self.quota {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "AWE node storage quota exceeded",
            ));
        }
        if let Err(error) = atomic_write_file(&p, data) {
            // Another writer may have published the same content-addressed
            // object concurrently. Accept it only after verifying its hash.
            if self.get(&id).is_ok() {
                return Ok(id);
            }
            return Err(error);
        }
        Ok(id)
    }
    pub fn get(&self, id: &[u8; 32]) -> io::Result<Vec<u8>> {
        let data = fs::read(self.path(id))?;
        if content_id(&data) != *id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "object integrity failure",
            ));
        }
        Ok(data)
    }
    pub fn remove(&self, id: &[u8; 32]) -> io::Result<()> {
        let _ = fs::remove_file(self.path(id));
        Ok(())
    }
    pub fn stats(&self) -> io::Result<NodeStorageStats> {
        let mut s = NodeStorageStats {
            capacity: self.quota,
            ..Default::default()
        };
        for e in fs::read_dir(self.root.join("objects"))? {
            let e = e?;
            if !e.file_type()?.is_file() {
                continue;
            }
            let name = e.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if hex::decode(name)
                .map(|bytes| bytes.len() == 32)
                .unwrap_or(false)
            {
                let metadata = e.metadata()?;
                s.used += metadata.len();
                s.objects += 1;
            }
        }
        Ok(s)
    }
    pub fn gc(&self, live: &BTreeMap<[u8; 32], ()>) -> io::Result<u64> {
        let mut removed = 0;
        for e in fs::read_dir(self.root.join("objects"))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            if let Ok(bytes) = hex::decode(name) {
                if bytes.len() == 32 {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(&bytes);
                    if !live.contains_key(&id) {
                        fs::remove_file(e.path())?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }
}

fn atomic_write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let mut opened = None;
    for _ in 0..8 {
        let mut nonce = [0u8; 16];
        OsRng.try_fill_bytes(&mut nonce).map_err(|error| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("secure temporary-file name generation failed: {error}"),
            )
        })?;
        let temporary_path = parent.join(format!(".awenet-state-{}.tmp", hex::encode(nonce)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary_path) {
            Ok(file) => {
                opened = Some((temporary_path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    let Some((temporary_path, mut file)) = opened else {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique AWENET temporary file",
        ));
    };

    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(file);

    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    // The file itself is durable before publication. Syncing the parent
    // directory is best-effort because not every supported filesystem allows it.
    #[cfg(unix)]
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Atomically publishes a manifest. The previous manifest remains intact if
/// serialization, temporary-file creation, writing, syncing, or rename fails.
pub fn save_manifest(path: impl AsRef<Path>, manifest: &[u8]) -> io::Result<()> {
    atomic_write_file(path.as_ref(), manifest)
}
pub fn load_manifest(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretFilePackage {
    pub sfid: String,
    pub filename: String,
    pub salt: [u8; 16],
    pub encrypted_payload: Vec<u8>,
}

impl SecretFilePackage {
    pub fn create(filename: &str, plain_data: &[u8], password: &str) -> io::Result<Self> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let key = crate::crypto::hash(password.as_bytes(), &salt);
        let encrypted_payload = encrypt_file(plain_data, &key)?;
        let sfid = format!(
            "sfid-{}",
            &hex::encode(crate::crypto::hash(b"AWE-SFID", &salt))[..16]
        );

        Ok(Self {
            sfid,
            filename: filename.to_string(),
            salt,
            encrypted_payload,
        })
    }

    pub fn unlock_and_decrypt(&self, password: &str) -> io::Result<Vec<u8>> {
        let key = crate::crypto::hash(password.as_bytes(), &self.salt);
        decrypt_file(&self.encrypted_payload, &key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_erasure_input_is_safe() {
        let p = StoragePolicy::default();
        let encoded = encode_shards(&[], &p).unwrap();
        assert_eq!(encoded.len(), p.total_shards());
    }

    #[test]
    fn encryption_roundtrip() {
        let key = [7u8; 32];
        let plain = b"AWE Drive secret";
        let enc = encrypt_file(plain, &key).unwrap();
        assert_ne!(enc, plain);
        assert_eq!(decrypt_file(&enc, &key).unwrap(), plain);
    }
    #[test]
    fn erasure_recovery() {
        let p = StoragePolicy::default();
        let data = vec![41u8; 100_000];
        let encoded = encode_shards(&data, &p).unwrap();
        let mut shards = encoded.into_iter().map(Some).collect::<Vec<_>>();
        shards[0] = None;
        shards[3] = None;
        shards[9] = None;
        let recovered = recover_shards(&mut shards, &p).unwrap();
        assert_eq!(&recovered[..data.len()], &data[..]);
    }

    #[test]
    fn hyper_sovereign_policy_spec() {
        let p = StoragePolicy::hyper_sovereign();
        assert_eq!(p.data_shards, 450);
        assert_eq!(p.parity_shards, 550);
        assert_eq!(p.data_shards + p.parity_shards, 1000);
        assert_eq!(p.replica_count, 3);

        let custom = StoragePolicy::custom_scaled(100_000_000);
        assert_eq!(custom.data_shards + custom.parity_shards, 100_000_000);
        assert_eq!(custom.replica_count, 3);
    }
    #[test]
    fn asmap_roundtrip() {
        let map = AsMap::new(
            [11u8; 32],
            "test.txt".to_string(),
            Some("site123".to_string()),
        );
        assert_eq!(map.total_shards, 1000);
        assert_eq!(map.shard_nodes.len(), 1000);
        assert!(map.shard_nodes.iter().all(|replicas| replicas.is_empty()));

        let mut map = map;
        map.assign_replicas(
            0,
            vec![
                "nid-real-a".into(),
                "nid-real-b".into(),
                "nid-real-c".into(),
            ],
        )
        .unwrap();
        assert_eq!(map.shard_nodes[0].len(), 3);
        assert!(map.assign_replicas(0, vec!["nid-real-a".into()]).is_err());
        let bytes = map.to_bytes().unwrap();
        let loaded = AsMap::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.filename, "test.txt");
        let nodes = (0..8).map(|i| format!("node-{i}")).collect::<Vec<_>>();
        let mut placed = loaded;
        placed.assign_all_replicas(&nodes).unwrap();
        assert!(placed.validate_complete_placement().is_ok());
    }
    #[test]
    fn daily_quota_tracker_limits() {
        let mut tracker = DailyQuotaTracker::default();
        let ok_upload = 10 * 1024 * 1024 * 1024; // 10 GB
        assert!(tracker.check_and_add(ok_upload).is_ok());
        assert_eq!(tracker.uploaded_today, ok_upload);

        let excessive_upload = 45 * 1024 * 1024 * 1024; // 45 GB (total 55 GB > 50 GB)
        assert!(tracker.check_and_add(excessive_upload).is_err());
    }
    #[test]
    fn content_addressed_store() {
        let dir = std::env::temp_dir().join(format!("awe-drive-{}", std::process::id()));
        let s = LocalNodeStore::open(&dir, 1_000_000).unwrap();
        let id = s.put(b"hello").unwrap();
        assert_eq!(s.get(&id).unwrap(), b"hello");
        s.remove(&id).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn secret_file_package_password_unlock() {
        let plain = b"Sovereign AWE Drive Secret File Data";
        let pwd = "SuperSecretPassword123";

        let sf_package = SecretFilePackage::create("secret_doc.pdf", plain, pwd).unwrap();
        assert!(sf_package.sfid.starts_with("sfid-"));

        // Correct password unlocks
        let decrypted = sf_package.unlock_and_decrypt(pwd).unwrap();
        assert_eq!(decrypted, plain);

        // Wrong password fails
        assert!(sf_package.unlock_and_decrypt("WrongPassword").is_err());
    }

    fn unique_storage_test_dir(label: &str) -> PathBuf {
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        std::env::temp_dir().join(format!(
            "awenet-{label}-{}-{}",
            std::process::id(),
            hex::encode(nonce)
        ))
    }

    #[test]
    fn manifest_save_replaces_existing_content_and_leaves_no_temp_file() {
        let dir = unique_storage_test_dir("manifest-replace");
        let path = dir.join("site.json");
        save_manifest(&path, b"{\"version\":1}").unwrap();
        save_manifest(&path, b"{\"version\":2}").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"{\"version\":2}");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn manifest_save_cleans_temp_file_when_publish_fails() {
        let dir = unique_storage_test_dir("manifest-failure");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("site.json");
        fs::create_dir(&path).unwrap();

        assert!(save_manifest(&path, b"manifest").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_manifest_writers_publish_only_complete_payloads() {
        let dir = unique_storage_test_dir("manifest-concurrent");
        let path = dir.join("site.json");
        let writers = (0..8)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let payload = format!("complete-writer-{index}");
                    save_manifest(&path, payload.as_bytes()).unwrap();
                    payload
                })
            })
            .collect::<Vec<_>>();
        let expected = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>();

        let published = fs::read_to_string(&path).unwrap();
        assert!(expected.iter().any(|candidate| candidate == &published));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_addressed_store_repairs_corrupt_existing_object() {
        let dir = unique_storage_test_dir("object-repair");
        let store = LocalNodeStore::open(&dir, 1_000_000).unwrap();
        let id = store.put(b"durable object").unwrap();
        fs::write(store.path(&id), b"corrupt bytes").unwrap();

        assert!(store.get(&id).is_err());
        assert_eq!(store.put(b"durable object").unwrap(), id);
        assert_eq!(store.get(&id).unwrap(), b"durable object");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn storage_stats_ignore_temporary_and_non_content_addressed_files() {
        let dir = unique_storage_test_dir("stats-filter");
        let store = LocalNodeStore::open(&dir, 1_000_000).unwrap();
        let objects = dir.join("objects");
        fs::write(objects.join(".awenet-state-interrupted.tmp"), b"temporary").unwrap();
        fs::write(objects.join("not-a-content-id"), b"junk").unwrap();

        store.put(b"count this object").unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.objects, 1);
        assert_eq!(stats.used, b"count this object".len() as u64);
        fs::remove_dir_all(dir).unwrap();
    } 

    #[test]
    fn concurrent_content_addressed_writers_publish_one_verified_object() {
        let dir = unique_storage_test_dir("object-concurrent");
        let store = std::sync::Arc::new(LocalNodeStore::open(&dir, 1_000_000).unwrap());
        let writers = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || store.put(b"shared durable object").unwrap())
            })
            .collect::<Vec<_>>();
        let ids = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>();

        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(store.get(&ids[0]).unwrap(), b"shared durable object");
        let stats = store.stats().unwrap();
        assert_eq!(stats.objects, 1);
        assert_eq!(stats.used, b"shared durable object".len() as u64);
        fs::remove_dir_all(dir).unwrap();
    }
}
