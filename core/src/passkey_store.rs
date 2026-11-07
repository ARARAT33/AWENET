//! Durable, founder-authenticated storage for passkey enrollment state.
//!
//! The passkey private key never enters this store. The persisted guard is
//! signed by the founder identity, and transient MFA challenge/session state is
//! stripped before signing so a restart cannot resurrect an authenticated UI.
use crate::{founder_guard::FounderGuard, identity::Identity};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const STORE_VERSION: u16 = 1;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SignedGuardFile {
    version: u16,
    guard: FounderGuard,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

fn canonical_guard(guard: &FounderGuard) -> Result<Vec<u8>, String> {
    serde_json::to_vec(guard)
        .map_err(|error| format!("passkey state serialization failed: {error}"))
}

fn private_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
    }
    #[cfg(not(unix))]
    {
        // Windows cannot open a directory handle for fsync; the atomic
        // rename is the durability boundary there, so the parent sync
        // stays a Unix-only step.
        let _ = path.parent();
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("passkeys"),
        std::process::id(),
        stamp ^ u128::from(sequence)
    );
    path.with_file_name(name)
}

/// Saves the non-ephemeral passkey state as a founder-signed atomic file.
pub fn save(path: &Path, guard: &FounderGuard, founder: &Identity) -> Result<(), String> {
    if guard.founder_awe_id() != &founder.public.awe_id {
        return Err("passkey guard founder does not match local identity".into());
    }
    let mut snapshot = guard.clone();
    snapshot.clear_transient_auth_state();
    let bytes = canonical_guard(&snapshot)?;
    let file = SignedGuardFile {
        version: STORE_VERSION,
        guard: snapshot,
        signature: founder.sign(&bytes),
    };
    let encoded = serde_json::to_vec_pretty(&file)
        .map_err(|error| format!("passkey store encoding failed: {error}"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("passkey store directory failed: {error}"))?;
    }
    let temporary = temp_path(path);
    let mut handle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("passkey temporary file failed: {error}"))?;
    if let Err(error) = private_file(&handle)
        .and_then(|_| handle.write_all(&encoded))
        .and_then(|_| handle.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!("passkey temporary file write failed: {error}"));
    }
    drop(handle);
    if let Err(error) = fs::rename(&temporary, path).and_then(|_| sync_parent(path)) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("passkey atomic replace failed: {error}"));
    }
    Ok(())
}

/// Loads and verifies a founder-signed passkey store. Ephemeral auth state is
/// always cleared, even if an older compatible store happened to contain it.
pub fn load(path: &Path, founder: &Identity) -> Result<FounderGuard, String> {
    let bytes = fs::read(path).map_err(|error| format!("passkey store read failed: {error}"))?;
    let file: SignedGuardFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("passkey store is invalid JSON: {error}"))?;
    if file.version != STORE_VERSION {
        return Err(format!(
            "unsupported passkey store version {}",
            file.version
        ));
    }
    if file.guard.founder_awe_id() != &founder.public.awe_id {
        return Err("passkey store belongs to a different founder".into());
    }
    let signed = canonical_guard(&file.guard)?;
    if !Identity::verify(&founder.public.public_key, &signed, &file.signature) {
        return Err("passkey store signature verification failed".into());
    }
    let mut guard = file.guard;
    guard.clear_transient_auth_state();
    Ok(guard)
}

/// Loads an existing store or installs and persists a fresh guard on first run.
pub fn load_or_install(
    path: &Path,
    founder: &Identity,
    at_unix: u64,
) -> Result<FounderGuard, String> {
    if path.exists() {
        load(path, founder)
    } else {
        let guard = FounderGuard::install(founder, at_unix);
        save(path, &guard, founder)?;
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{founder_guard::FounderGuard, identity::Username};

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("awe-passkey-store-{name}-{}", std::process::id()))
    }

    #[test]
    fn signed_store_roundtrips_and_clears_transient_auth() {
        let founder = identity("store-founder");
        let credential = identity("store-passkey");
        let file = path("roundtrip");
        let mut guard = FounderGuard::install(&founder, 10);
        let enrollment = founder.sign(&FounderGuard::enrollment_message(
            &credential.public.public_key,
        ));
        guard
            .enroll_passkey(&enrollment, &credential, "laptop", 11)
            .unwrap();
        let nonce = guard.begin_step_up();
        let assertion = credential.sign(&FounderGuard::step_up_message(&nonce));
        guard
            .complete_step_up(&credential.public.public_key, &assertion, 12)
            .unwrap();
        save(&file, &guard, &founder).unwrap();
        let loaded = load(&file, &founder).unwrap();
        assert_eq!(loaded.passkeys(), guard.passkeys());
        assert!(loaded.session().is_none());
        assert!(!loaded.session_active(12));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn tampered_store_and_wrong_founder_are_rejected() {
        let founder = identity("tamper-founder");
        let wrong = identity("wrong-founder");
        let file = path("tamper");
        save(&file, &FounderGuard::install(&founder, 0), &founder).unwrap();
        let mut bytes = fs::read(&file).unwrap();
        let index = bytes.len() - 2;
        bytes[index] = if bytes[index] == b'0' { b'1' } else { b'0' };
        fs::write(&file, bytes).unwrap();
        assert!(load(&file, &founder).is_err());
        save(&file, &FounderGuard::install(&founder, 0), &founder).unwrap();
        assert!(load(&file, &wrong).is_err());
        let _ = fs::remove_file(file);
    }
}
