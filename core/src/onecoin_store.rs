//! Durable ONECOIN ledger storage and transaction admission helpers.
use crate::{onecoin::{OnecoinLedger, OnecoinTransaction}, onecoin_consensus::{OnecoinBlock, OnecoinFinalizedState, QuorumCertificate}};
use serde::{de::DeserializeOwned, Serialize};
use std::{collections::BTreeMap, fs, io, path::{Path, PathBuf}};

const MAGIC: &[u8] = b"AWENET-ONECOIN-LEDGER-V1\0";
const MAX_LEDGER_BYTES: usize = 64 * 1024 * 1024;
const STATE_MAGIC: &[u8] = b"AWENET-ONECOIN-STATE-V1\0";

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("part");
    use std::io::Write;
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

fn load_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    if bytes.len() > MAX_LEDGER_BYTES || bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid AWENET ledger file"));
    }
    serde_json::from_slice(&bytes[MAGIC.len()..])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "corrupt AWENET ledger state"))
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let body = serde_json::to_vec(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ledger serialization failed"))?;
    if body.len() > MAX_LEDGER_BYTES - MAGIC.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ledger state too large"));
    }
    let mut bytes = Vec::with_capacity(MAGIC.len() + body.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&body);
    atomic_write(path, &bytes)
}

#[derive(Clone, Debug)]
pub struct PersistentOnecoinLedger {
    path: PathBuf,
    pub ledger: OnecoinLedger,
}

impl PersistentOnecoinLedger {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() { fs::create_dir_all(parent)?; }
        }
        if path.exists() {
            let ledger: OnecoinLedger = load_json(&path)?;
            Self::validate(&ledger)?;
            Ok(Self { path, ledger })
        } else {
            Ok(Self { path, ledger: OnecoinLedger::default() })
        }
    }

    pub fn save(&self) -> io::Result<()> { save_json(&self.path, &self.ledger) }

    pub fn apply_transfer(&mut self, tx: &OnecoinTransaction, sender_public_key: &[u8; 32]) -> Result<[u8; 32], String> {
        let id = self.ledger.apply_transfer(tx, sender_public_key)?;
        self.save().map_err(|e| format!("failed to persist ONECOIN transaction: {e}"))?;
        Ok(id)
    }

    pub fn flush(&self) -> io::Result<()> { self.save() }

    fn validate(ledger: &OnecoinLedger) -> io::Result<()> {
        if ledger.members.len() != ledger.balances.len() || ledger.members.len() != ledger.nonces.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ledger member state is inconsistent"));
        }
        let computed = ledger.balances.values().copied().fold(0u128, u128::saturating_add);
        if computed > ledger.total_issued_atoms {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ledger supply invariant violated"));
        }
        for (id, public_key) in &ledger.members {
            if crate::identity::AweId::from_public_key(public_key).to_hex() != *id {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ledger AWEID/public-key mismatch"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Username};

    #[test]
    fn ledger_survives_atomic_reload() {
        let dir = std::env::temp_dir().join(format!("awe-onecoin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");
        let a = Identity::generate(Username::new("alice").unwrap());
        let b = Identity::generate(Username::new("bob").unwrap());
        let mut p = PersistentOnecoinLedger::open(&path).unwrap();
        p.ledger.initialize_genesis(&[a.public.awe_id.clone(), b.public.awe_id.clone()]).unwrap();
        p.save().unwrap();
        let q = PersistentOnecoinLedger::open(&path).unwrap();
        assert_eq!(q.ledger.balance_atoms(&a.public.awe_id), crate::onecoin::INITIAL_GENESIS_ALLOCATION);
        let _ = fs::remove_dir_all(&dir);
    }
}


#[derive(Clone, Debug)]
pub struct PersistentOnecoinState {
    path: PathBuf,
    pub state: OnecoinFinalizedState,
}

impl PersistentOnecoinState {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() { fs::create_dir_all(parent)?; }
        }
        if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.len() > MAX_LEDGER_BYTES
                || bytes.len() < STATE_MAGIC.len()
                || &bytes[..STATE_MAGIC.len()] != STATE_MAGIC
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid AWENET ONECOIN state file"));
            }
            let state: OnecoinFinalizedState = serde_json::from_slice(&bytes[STATE_MAGIC.len()..])
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "corrupt AWENET ONECOIN state"))?;
            if state.ledger.members.len() != state.ledger.balances.len()
                || state.ledger.members.len() != state.ledger.nonces.len()
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ONECOIN state member invariant violated"));
            }
            Ok(Self { path, state })
        } else {
            Ok(Self { path, state: OnecoinFinalizedState::default() })
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let body = serde_json::to_vec(&self.state)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ONECOIN state serialization failed"))?;
        if body.len() > MAX_LEDGER_BYTES - STATE_MAGIC.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ONECOIN state too large"));
        }
        let mut bytes = Vec::with_capacity(STATE_MAGIC.len() + body.len());
        bytes.extend_from_slice(STATE_MAGIC);
        bytes.extend_from_slice(&body);
        atomic_write(&self.path, &bytes)
    }

    pub fn finalize(
        &mut self,
        block: &OnecoinBlock,
        certificate: &QuorumCertificate,
        validators: &BTreeMap<String, [u8; 32]>,
    ) -> Result<[u8; 32], String> {
        let hash = self.state.finalize(block, certificate, validators)?;
        self.save().map_err(|e| format!("failed to persist finalized ONECOIN state: {e}"))?;
        Ok(hash)
    }
}
