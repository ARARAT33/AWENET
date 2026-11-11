use crate::{
    access::{AccessDescriptor, AccessMap},
    namespace,
    registry::{Registry, RegistryStatus},
    storage::{content_id, LocalNodeStore},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPolicy {
    pub max_site_bytes: u64,
    pub max_file_bytes: u64,
    pub bandwidth_bytes_per_second: u64,
    pub cache_entries: usize,
    pub replica_count: usize,
}
impl Default for HostPolicy {
    fn default() -> Self {
        Self {
            max_site_bytes: 1 << 30,
            max_file_bytes: 64 << 20,
            bandwidth_bytes_per_second: 10 << 20,
            cache_entries: 4096,
            replica_count: 3,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostedFile {
    pub path: String,
    pub object_id: [u8; 32],
    pub size: u64,
    pub content_type: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteManifest {
    pub domain: String,
    pub version: u64,
    pub root_hash: [u8; 32],
    pub files: Vec<HostedFile>,
    pub owner_key: Vec<u8>,
    pub policy: HostPolicy,
    #[serde(default = "default_host_access")]
    pub access: AccessDescriptor,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostHealth {
    pub available: bool,
    pub consecutive_failures: u32,
    pub last_check_unix: u64,
    pub latency_ms: u64,
}
fn default_host_access() -> AccessDescriptor {
    AccessDescriptor::open()
}

impl SiteManifest {
    pub fn open(&self) -> bool {
        self.access.mode == crate::access::AccessMode::Open
    }
    pub fn authorize(&self, map: Option<&AccessMap>) -> bool {
        self.access.authorize(map, &self.root_hash)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRecord {
    pub node_id: [u8; 32],
    pub domain: String,
    pub version: u64,
    pub manifest_hash: [u8; 32],
    pub health: HostHealth,
}

pub struct AweHost {
    root: PathBuf,
    store: LocalNodeStore,
    policy: HostPolicy,
    cache: BTreeMap<([u8; 32], String), Vec<u8>>,
}
impl AweHost {
    pub fn open(root: impl AsRef<Path>, quota: u64, policy: HostPolicy) -> io::Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            store: LocalNodeStore::open(root.as_ref().join("objects"), quota)?,
            policy,
            cache: BTreeMap::new(),
        })
    }
    pub fn publish_file(
        &mut self,
        path: &str,
        data: &[u8],
        content_type: &str,
    ) -> io::Result<HostedFile> {
        let clean = normalize_path(path)?;
        if data.len() as u64 > self.policy.max_file_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file exceeds host policy",
            ));
        }
        let id = self.store.put(data)?;
        Ok(HostedFile {
            path: clean,
            object_id: id,
            size: data.len() as u64,
            content_type: content_type.to_string(),
        })
    }
    pub fn publish_manifest(
        &self,
        domain: &str,
        version: u64,
        owner_key: Vec<u8>,
        files: Vec<HostedFile>,
    ) -> io::Result<SiteManifest> {
        if !crate::registry::valid_name(domain) || version == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid site manifest",
            ));
        }
        let total = files
            .iter()
            .try_fold(0u64, |total, file| total.checked_add(file.size))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "site size overflow"))?;
        if total > self.policy.max_site_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "site exceeds host policy",
            ));
        }
        let mut h = Sha256::new();
        h.update(domain.as_bytes());
        h.update(version.to_be_bytes());
        for f in &files {
            h.update(f.path.as_bytes());
            h.update(f.object_id);
            h.update(f.size.to_be_bytes());
        }
        Ok(SiteManifest {
            domain: domain.to_string(),
            version,
            root_hash: h.finalize().into(),
            files,
            owner_key,
            policy: self.policy.clone(),
            access: AccessDescriptor::open(),
        })
    }
    pub fn authorize_domain(manifest: &SiteManifest, registry: &Registry) -> bool {
        namespace::resolve(registry, &manifest.domain)
            .map(|r| r.status == RegistryStatus::Active && r.owner_public_key == manifest.owner_key)
            .unwrap_or(false)
    }
    pub fn get_authorized(
        &mut self,
        manifest: &SiteManifest,
        path: &str,
        map: Option<&AccessMap>,
    ) -> io::Result<Vec<u8>> {
        if !manifest.authorize(map) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "AWE site access denied",
            ));
        }
        self.get(manifest, path)
    }

    pub fn get(&mut self, manifest: &SiteManifest, path: &str) -> io::Result<Vec<u8>> {
        let clean = normalize_path(path)?;
        // A path is unique only within a site; include the manifest identity
        // so one site's cached bytes can never be served for another site.
        let cache_key = (manifest.root_hash, clean.clone());
        if let Some(v) = self.cache.get(&cache_key) {
            return Ok(v.clone());
        }
        let f = manifest
            .files
            .iter()
            .find(|f| f.path == clean)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "AWE host path not found"))?;
        let data = self.store.get(&f.object_id)?;
        if data.len() as u64 != f.size || content_id(&data) != f.object_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host object integrity failure",
            ));
        }
        if self.cache.len() >= self.policy.cache_entries {
            if let Some(k) = self.cache.keys().next().cloned() {
                self.cache.remove(&k);
            }
        }
        self.cache.insert(cache_key, data.clone());
        Ok(data)
    }
    pub fn save_manifest(&self, manifest: &SiteManifest) -> io::Result<()> {
        if !crate::registry::valid_name(&manifest.domain) || manifest.version == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid site manifest identity",
            ));
        }
        let path = self.manifest_path(&manifest.domain);
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "manifest serialization failed")
        })?;
        crate::storage::save_manifest(path, &bytes)
    }
    pub fn load_manifest(&self, domain: &str) -> io::Result<SiteManifest> {
        if !crate::registry::valid_name(domain) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid site domain",
            ));
        }
        let bytes = fs::read(self.manifest_path(domain))?;
        let manifest: SiteManifest = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid host manifest"))?;
        if !crate::registry::valid_name(&manifest.domain)
            || manifest.version == 0
            || !manifest.domain.eq_ignore_ascii_case(domain)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host manifest identity mismatch",
            ));
        }
        Ok(manifest)
    }
    pub fn manifest_path(&self, domain: &str) -> PathBuf {
        // Valid site names are already constrained to DNS-safe ASCII labels.
        // Hash invalid input so even direct callers cannot escape the host root.
        let filename = if crate::registry::valid_name(domain) {
            domain.to_ascii_lowercase()
        } else {
            format!("invalid-{}", hex::encode(Sha256::digest(domain.as_bytes())))
        };
        self.root.join(format!("{filename}.manifest.json"))
    }
}
pub fn normalize_path(path: &str) -> io::Result<String> {
    if path.is_empty()
        || !path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|x| x == "..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid web path",
        ));
    }
    Ok(if path == "/" {
        "/index.html".into()
    } else {
        path.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_paths() {
        assert!(normalize_path("/index.html").is_ok());
        assert!(normalize_path("/../secret").is_err())
    }
    #[test]
    fn site_domains_cannot_escape_the_host_root() {
        let root = std::env::temp_dir().join(format!("awe-host-path-{}", std::process::id()));
        let host = AweHost::open(&root, 10_000_000, HostPolicy::default()).unwrap();

        assert!(host
            .publish_manifest("../outside", 1, vec![1], Vec::new())
            .is_err());
        assert!(host.load_manifest("../outside").is_err());
        assert!(host.load_manifest("..\\\\outside").is_err());
        assert!(host.manifest_path("../outside").starts_with(&root));
        assert!(host.manifest_path("..\\\\outside").starts_with(&root));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_manifest_rejects_a_different_embedded_domain() {
        let root =
            std::env::temp_dir().join(format!("awe-host-domain-mismatch-{}", std::process::id()));
        let host = AweHost::open(&root, 10_000_000, HostPolicy::default()).unwrap();
        let manifest = host
            .publish_manifest("example.awe", 1, vec![1], Vec::new())
            .unwrap();
        let mut tampered = manifest;
        tampered.domain = "other.awe".into();
        fs::write(
            host.manifest_path("example.awe"),
            serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();

        let error = host.load_manifest("example.awe").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn site_cache_is_isolated_by_manifest_identity() {
        let root =
            std::env::temp_dir().join(format!("awe-host-cache-isolation-{}", std::process::id()));
        let mut host = AweHost::open(&root, 10_000_000, HostPolicy::default()).unwrap();

        let first_file = host
            .publish_file("/index.html", b"first site", "text/html")
            .unwrap();
        let first = host
            .publish_manifest("first.awe", 1, vec![1], vec![first_file])
            .unwrap();
        let second_file = host
            .publish_file("/index.html", b"second site", "text/html")
            .unwrap();
        let second = host
            .publish_manifest("second.awe", 1, vec![2], vec![second_file])
            .unwrap();

        assert_eq!(host.get(&first, "/").unwrap(), b"first site");
        assert_eq!(host.get(&second, "/").unwrap(), b"second site");
        assert_eq!(host.get(&first, "/index.html").unwrap(), b"first site");
        assert_eq!(host.get(&second, "/index.html").unwrap(), b"second site");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn site_size_overflow_is_rejected_without_panicking() {
        let root =
            std::env::temp_dir().join(format!("awe-host-size-overflow-{}", std::process::id()));
        let host = AweHost::open(&root, 10_000_000, HostPolicy::default()).unwrap();
        let files = vec![
            HostedFile {
                path: "/large.bin".into(),
                object_id: [1; 32],
                size: u64::MAX,
                content_type: "application/octet-stream".into(),
            },
            HostedFile {
                path: "/small.bin".into(),
                object_id: [2; 32],
                size: 1,
                content_type: "application/octet-stream".into(),
            },
        ];

        let error = host
            .publish_manifest("overflow.awe", 1, vec![1], files)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn host_roundtrip() {
        let r = std::env::temp_dir().join(format!("awe-host-{}", std::process::id()));
        let mut h = AweHost::open(&r, 10_000_000, HostPolicy::default()).unwrap();
        let f = h
            .publish_file("/index.html", b"<h1>AWE</h1>", "text/html")
            .unwrap();
        let m = h
            .publish_manifest("example.awe", 1, vec![1], vec![f])
            .unwrap();
        h.save_manifest(&m).unwrap();
        let loaded = h.load_manifest("example.awe").unwrap();
        assert_eq!(loaded, m);
        assert!(m.open());
        assert_eq!(h.get_authorized(&m, "/", None).unwrap(), b"<h1>AWE</h1>");
        let _ = fs::remove_dir_all(r);
    }
}
