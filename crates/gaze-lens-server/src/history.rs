//! Durable identity enrollment; grants are deliberately outside this history.
use crate::{auth::Authority, hex, private};
use gaze_lens_protocol::{Error, Result, bounds};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Principal,
    Resource,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) kind: Kind,
    pub(crate) id: String,
    pub(crate) generation: String,
    pub(crate) fingerprint: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    identity: Identity,
    active: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u8,
    entries: Vec<Entry>,
}

/// Holds an exclusive process lock and the exact identities enrolled at startup.
/// Open on a blocking worker, before accepting any connections.
pub struct History {
    _lock: File,
    enrolled: Vec<Identity>,
}
impl History {
    pub fn open(path: &Path, authority: &Authority) -> Result<Self> {
        private::directory(path)?;
        let mut lock_name = path.as_os_str().to_owned();
        lock_name.push(".lock");
        let lock = lock(&PathBuf::from(lock_name))?;
        lock.try_lock().map_err(|_| Error::Unavailable)?;
        let enrolled = authority.identities()?;
        let mut ledger = read(path)?;
        let changed = ledger.enroll(&enrolled)?;
        if changed {
            persist(path, &ledger)?;
        }
        Ok(Self {
            _lock: lock,
            enrolled,
        })
    }
    /// Validate a proposed restart without mutating history or acquiring sources.
    pub fn check(path: &Path, authority: &Authority) -> Result<()> {
        private::directory(path)?;
        read(path)?.enroll(&authority.identities()?).map(|_| ())
    }
    /// Identity definitions are immutable for a running server; grants stay live.
    pub fn validate(&self, authority: &Authority) -> Result<()> {
        if self.enrolled == authority.identities()? {
            Ok(())
        } else {
            Err(Error::BindingChanged)
        }
    }
}
impl Ledger {
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Unavailable);
        }
        bounds::cap(self.entries.len(), 1024)?;
        for (i, e) in self.entries.iter().enumerate() {
            let v = &e.identity;
            if !hex(&v.id, 32) || !hex(&v.generation, 32) || !hex(&v.fingerprint, 64) {
                return Err(Error::Unavailable);
            }
            for old in &self.entries[..i] {
                if old.identity.generation == v.generation
                    || (old.identity.id == v.id && (old.identity.kind != v.kind || old.active))
                {
                    return Err(Error::Unavailable);
                }
            }
        }
        Ok(())
    }
    fn enroll(&mut self, requested: &[Identity]) -> Result<bool> {
        self.validate()?;
        let mut changed = false;
        for id in requested {
            let previous = self.entries.iter().rposition(|e| e.identity.id == id.id);
            if let Some(index) = previous {
                let old = &self.entries[index];
                if !old.active || old.identity.kind != id.kind {
                    return Err(Error::BindingChanged);
                }
                if old.identity.generation == id.generation {
                    if old.identity != *id {
                        return Err(Error::BindingChanged);
                    }
                    continue;
                }
            }
            if self
                .entries
                .iter()
                .any(|e| e.identity.generation == id.generation)
            {
                return Err(Error::BindingChanged);
            }
            if let Some(index) = previous {
                self.entries[index].active = false;
            }
            bounds::cap(self.entries.len() + 1, 1024)?;
            self.entries.push(Entry {
                identity: id.clone(),
                active: true,
            });
            changed = true;
        }
        for entry in &mut self.entries {
            if entry.active && !requested.iter().any(|v| v.id == entry.identity.id) {
                entry.active = false;
                changed = true;
            }
        }
        // The 64 KiB serialized ceiling, not the 1,024-entry count cap, is the
        // limit that binds first; see docs/phase2-proof.md.
        bounds::serialized_size(self, private::FILE_BYTES)?;
        Ok(changed)
    }
}
/// Creates the lock exclusively, or reopens exactly the private file already
/// there. `create_new` cannot follow a planted symlink, and the reopen path
/// verifies the type, privacy, owner and inode the same way the reader does.
fn lock(path: &Path) -> Result<File> {
    private::directory(path)?;
    let mut fresh = OpenOptions::new();
    fresh.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fresh.mode(0o600);
    }
    match fresh.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private::open(path, OpenOptions::new().read(true).write(true))
        }
        Err(_) => Err(Error::Unavailable),
    }
}
fn read(path: &Path) -> Result<Ledger> {
    let bytes = private::read(path)?;
    bounds::json(&bytes, private::FILE_BYTES)?;
    let ledger: Ledger = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
    ledger.validate()?;
    Ok(ledger)
}
fn persist(path: &Path, ledger: &Ledger) -> Result<()> {
    bounds::serialized_size(ledger, private::FILE_BYTES)?;
    let directory = private::directory(path)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| Error::Unavailable)?;
    serde_json::to_writer(&mut temporary, ledger).map_err(|_| Error::Unavailable)?;
    temporary.flush().map_err(|_| Error::Unavailable)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| Error::Unavailable)?;
    temporary.persist(path).map_err(|_| Error::Unavailable)?;
    File::open(directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Unavailable)
}
