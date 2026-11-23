//! Gateway-only encrypted records, durable refresh markers and cross-process ownership.

use super::{Error, auth::Bundle};
use crate::secrets::{Aad, TokenCipher, sha256};
use contracts::{gateway::RelinkReason, ids::UserId, job::result::Usage, runtime::ContainerId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Record {
    pub revision: u64,
    pub generation: u64,
    pub bundle: Option<Bundle>,
    pub inflight: bool,
    pub reason: Option<RelinkReason>,
    pub link_attempt: Option<(uuid::Uuid, i64)>,
    pub last_call: Option<contracts::job::result::LlmCallOutcome>,
    pub keys: BTreeMap<String, Key>,
    pub responses: BTreeMap<String, Meter>,
    pub limits: BTreeMap<String, String>,
    pub resets_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Key {
    pub container: ContainerId,
    pub expires_at: i64,
    pub revoked: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Meter {
    pub key: String,
    pub usage: Usage,
}

pub(super) struct Store {
    root: PathBuf,
    cipher: TokenCipher,
    #[cfg(test)]
    pub fail_after_marker: std::sync::atomic::AtomicBool,
}

pub(super) struct Locked {
    store: Arc<Store>,
    user: UserId,
    _owner: File,
    pub record: Record,
}

pub(super) fn digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

impl Store {
    pub fn new(root: &Path, cipher: TokenCipher) -> Result<Arc<Self>, Error> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Configuration);
        }
        for name in ["users", "keys"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(name))
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })?;
        }
        for path in [root.to_path_buf(), root.join("users"), root.join("keys")] {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(Error::Configuration);
            }
        }
        File::open(root)?.sync_all()?;
        Ok(Arc::new(Self {
            root: root.to_path_buf(),
            cipher,
            #[cfg(test)]
            fail_after_marker: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    fn user_path(&self, user: &UserId, suffix: &str) -> PathBuf {
        self.root
            .join("users")
            .join(format!("{}.{suffix}", digest(user.0.as_bytes())))
    }

    pub fn record_path(&self, user: &UserId) -> PathBuf {
        self.user_path(user, "sealed")
    }

    pub async fn lock(self: &Arc<Self>, user: &UserId) -> Result<Locked, Error> {
        let path = self.user_path(user, "lock");
        let owner = tokio::task::spawn_blocking(move || -> Result<File, std::io::Error> {
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)?;
            f.lock()?;
            Ok(f)
        })
        .await
        .map_err(Error::Join)??;
        let mut locked = Locked {
            store: self.clone(),
            user: user.clone(),
            _owner: owner,
            record: Record::default(),
        };
        locked.record = locked.load()?;
        // Cancellation/crash after sending is indistinguishable from consumption. Never replay.
        if (locked.record.inflight || locked.store.user_path(&locked.user, "pending").exists())
            && locked.record.reason.is_none()
        {
            locked.record.reason = Some(RelinkReason::Quarantined);
            locked.record.inflight = false;
            locked.save()?;
        }
        Ok(locked)
    }

    pub fn index_key(&self, id: &str, user: &UserId) -> Result<(), Error> {
        let bytes = self.cipher.encrypt(
            Aad {
                table: "gateway_keys",
                column: "payer",
                row_id: id,
            },
            &serde_json::to_string(user)?,
        )?;
        atomic(&self.root.join("keys").join(id), &bytes)
    }

    pub fn key_user(&self, id: &str) -> Result<UserId, Error> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Key);
        }
        let bytes = fs::read(self.root.join("keys").join(id)).map_err(|_| Error::Key)?;
        let clear = self.cipher.decrypt(
            Aad {
                table: "gateway_keys",
                column: "payer",
                row_id: id,
            },
            &bytes,
        )?;
        Ok(serde_json::from_str(clear.expose_secret())?)
    }

    pub async fn meter(
        self: &Arc<Self>,
        user: &UserId,
        key: &str,
        response: &str,
        usage: Usage,
    ) -> Result<(), Error> {
        usage
            .checked_add(Usage::default())
            .map_err(|_| Error::Usage)?;
        let mut tx = self.lock(user).await?;
        if !tx.record.responses.contains_key(response) {
            tx.record.responses.insert(
                response.into(),
                Meter {
                    key: key.into(),
                    usage,
                },
            );
            tx.record.last_call = Some(contracts::job::result::LlmCallOutcome {
                at: contracts::time::Timestamp::now(),
                status: contracts::job::result::LlmCallStatus::Working,
            });
            tx.save()?;
        }
        Ok(())
    }
}

impl Locked {
    fn load(&self) -> Result<Record, Error> {
        let path = self.store.record_path(&self.user);
        let witness = self.store.user_path(&self.user, "witness");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !witness.exists() => {
                return Ok(Record::default());
            }
            Err(e) => return Err(Error::Io(e)),
        };
        let receipt: (u64, String) = fs::read(witness)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or((0, String::new()));
        let user = self.user.to_string();
        let clear = self.store.cipher.decrypt(
            Aad {
                table: "gateway",
                column: "record",
                row_id: &user,
            },
            &bytes,
        )?;
        let mut record: Record = serde_json::from_str(clear.expose_secret())?;
        // The independent high-water witness refuses an older snapshot; no backup fallback exists.
        if receipt.1 != digest(&bytes) || receipt.0 != record.revision {
            record.reason = Some(RelinkReason::Quarantined);
            record.revision = record.revision.max(receipt.0);
            record.generation = record.generation.max(receipt.0);
        }
        Ok(record)
    }

    pub fn begin_refresh(&self) -> Result<(), Error> {
        atomic(
            &self.store.user_path(&self.user, "pending"),
            &self.record.generation.to_be_bytes(),
        )
    }

    pub fn finish_refresh(&self) -> Result<(), Error> {
        match fs::remove_file(self.store.user_path(&self.user, "pending")) {
            Ok(()) => File::open(self.store.root.join("users"))?
                .sync_all()
                .map_err(Error::Io),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub fn save(&mut self) -> Result<(), Error> {
        self.record.revision = self.record.revision.checked_add(1).ok_or(Error::Usage)?;
        let user = self.user.to_string();
        let bytes = self.store.cipher.encrypt(
            Aad {
                table: "gateway",
                column: "record",
                row_id: &user,
            },
            &serde_json::to_string(&self.record)?,
        )?;
        atomic(&self.store.record_path(&self.user), &bytes)?;
        atomic(
            &self.store.user_path(&self.user, "witness"),
            &serde_json::to_vec(&(self.record.revision, digest(&bytes)))?,
        )
    }
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), Error> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        let parent = path.parent().ok_or(Error::Configuration)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
