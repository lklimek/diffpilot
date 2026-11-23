//! Gateway-only encrypted records, durable refresh markers and cross-process ownership.

use super::{Error, auth::Bundle, now};
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
    #[serde(default)]
    pub last_auth: Option<AuthOutcome>,
    #[serde(default)]
    pub archived_usage: Usage,
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
    #[serde(default)]
    pub archived_usage: Usage,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Meter {
    pub key: String,
    pub usage: Usage,
    #[serde(default = "now")]
    pub recorded_at: i64,
}

// Seven days covers delayed worker reconciliation; lifetime payer totals survive pruning.
pub(super) const RETENTION: i64 = 7 * 86400;
const MAX_KEYS: usize = 4096;
const MAX_RESPONSES: usize = 65536;

#[derive(Serialize, Deserialize)]
pub(super) struct AuthOutcome {
    at: i64,
    generation: u64,
    operation: String,
    outcome: String,
    status: Option<u16>,
    code: Option<String>,
    request_id: Option<String>,
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
    outcome_changed: bool,
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
            outcome_changed: false,
        };
        let (record, rolled_back) = locked.load().inspect_err(Error::log_storage)?;
        locked.record = record;
        if rolled_back {
            locked.save()?;
        }
        let pending = fs::read(locked.store.user_path(&locked.user, "pending"));
        let pending = match pending {
            Ok(bytes) => Some(bytes.try_into().ok().map(u64::from_be_bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(Error::Io(error)),
        };
        let resolved = pending.is_some_and(|generation| {
            generation.is_some_and(|generation| {
                locked.record.generation > generation
                    || (locked.record.generation == generation
                        && locked.record.last_auth.as_ref().is_some_and(|e| {
                            e.generation == generation
                                && e.operation == "refresh"
                                && e.outcome == "pre_send_failed"
                        }))
            })
        }) && !locked.record.inflight;
        if resolved {
            locked.finish_refresh()?;
        }
        // A settled successor resolves its predecessor's marker in the same durable generation.
        if (locked.record.inflight || (pending.is_some() && !resolved))
            && locked.record.reason.is_none()
        {
            locked.record.reason = Some(RelinkReason::Quarantined);
            locked.record.inflight = false;
            locked.outcome(
                "recovery",
                "quarantined",
                None,
                Some("unresolved_marker"),
                None,
            );
            locked.save()?;
        }
        locked.prune()?;
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

    pub fn retire_key(&self, id: &str) -> Result<(), Error> {
        atomic(&self.root.join("keys").join(format!("{id}.retired")), &[])
            .inspect_err(Error::log_storage)
    }

    pub fn key_live(&self, id: &str) -> bool {
        self.root.join("keys").join(id).exists()
            && !self
                .root
                .join("keys")
                .join(format!("{id}.retired"))
                .exists()
    }

    pub fn key_user(&self, id: &str) -> Result<UserId, Error> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Key);
        }
        let bytes = fs::read(self.root.join("keys").join(id))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::Key
                } else {
                    Error::Io(error)
                }
            })
            .inspect_err(Error::log_storage)?;
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
            if tx.record.responses.len() >= MAX_RESPONSES || response.len() > 256 {
                return Err(Error::Busy);
            }
            tx.record.responses.insert(
                response.into(),
                Meter {
                    key: key.into(),
                    usage,
                    recorded_at: now(),
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
    fn load(&self) -> Result<(Record, bool), Error> {
        let path = self.store.record_path(&self.user);
        // The fsynced redo journal closes the record/witness rename window without accepting rollback.
        self.recover_commit()?;
        let witness = self.store.user_path(&self.user, "witness");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !witness.exists() => {
                return Ok((Record::default(), false));
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
        let rolled_back = receipt.1 != digest(&bytes) || receipt.0 != record.revision;
        if rolled_back {
            tracing::warn!(
                payer = digest(self.user.0.as_bytes()),
                generation = record.generation,
                outcome = "quarantined",
                code = "witness_mismatch",
                "gateway rejected a rolled-back credential record"
            );
            record.last_auth = Some(AuthOutcome {
                at: now(),
                generation: record.generation,
                operation: "recovery".into(),
                outcome: "quarantined".into(),
                status: None,
                code: Some("witness_mismatch".into()),
                request_id: None,
            });
            record.reason = Some(RelinkReason::Quarantined);
            record.revision = record.revision.max(receipt.0);
            record.generation = record.generation.max(receipt.0);
        }
        Ok((record, rolled_back))
    }

    pub fn begin_refresh(&self) -> Result<(), Error> {
        atomic(
            &self.store.user_path(&self.user, "pending"),
            &self.record.generation.to_be_bytes(),
        )
        .inspect_err(Error::log_storage)
    }

    pub fn finish_refresh(&self) -> Result<(), Error> {
        remove(&self.store.user_path(&self.user, "pending")).inspect_err(Error::log_storage)
    }

    pub fn save(&mut self) -> Result<(), Error> {
        let result = self.save_inner().inspect_err(Error::log_storage);
        if self.outcome_changed {
            if let Some(evidence) = &self.record.last_auth {
                self.log_outcome(evidence, result.is_ok());
            }
            if result.is_ok() {
                self.outcome_changed = false;
            }
        }
        result
    }

    fn save_inner(&mut self) -> Result<(), Error> {
        self.record.revision = self.record.revision.checked_add(1).ok_or(Error::Usage)?;
        let user = self.user.to_string();
        let bytes = self.store.cipher.encrypt(
            Aad {
                table: "gateway",
                column: "record",
                row_id: &user,
            },
            secrecy::zeroize::Zeroizing::new(serde_json::to_string(&self.record)?).as_str(),
        )?;
        let mut journal = self.record.revision.to_be_bytes().to_vec();
        journal.extend_from_slice(&bytes);
        atomic(&self.store.user_path(&self.user, "commit"), &journal)?;
        atomic(&self.store.record_path(&self.user), &bytes)?;
        atomic(
            &self.store.user_path(&self.user, "witness"),
            &serde_json::to_vec(&(self.record.revision, digest(&bytes)))?,
        )?;
        remove(&self.store.user_path(&self.user, "commit"))
    }
    #[cfg(test)]
    pub fn fail_after_marker(&self) -> bool {
        self.store
            .fail_after_marker
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn outcome(
        &mut self,
        operation: &str,
        outcome: &str,
        status: Option<u16>,
        code: Option<&str>,
        request_id: Option<String>,
    ) {
        let evidence = AuthOutcome {
            at: now(),
            generation: self.record.generation,
            operation: operation.into(),
            outcome: outcome.into(),
            status,
            code: code.map(str::to_owned),
            request_id,
        };
        self.record.last_auth = Some(evidence);
        self.outcome_changed = true;
    }

    fn log_outcome(&self, evidence: &AuthOutcome, durable: bool) {
        let AuthOutcome {
            at,
            generation,
            operation,
            outcome,
            status,
            code,
            request_id,
        } = evidence;
        if durable
            && matches!(
                outcome.as_str(),
                "committed" | "revoked" | "locally_unlinked"
            )
        {
            tracing::info!(
                payer = digest(self.user.0.as_bytes()),
                generation,
                at,
                operation,
                outcome,
                status,
                code,
                request_id,
                "gateway credential lifecycle settled"
            );
        } else {
            tracing::warn!(
                payer = digest(self.user.0.as_bytes()),
                generation,
                at,
                operation,
                outcome,
                status,
                code,
                request_id,
                durable,
                "gateway credential lifecycle needs attention"
            );
        }
    }

    pub fn key_capacity(&self) -> Result<(), Error> {
        if self.record.keys.len() >= MAX_KEYS {
            Err(Error::Busy)
        } else {
            Ok(())
        }
    }

    fn prune(&mut self) -> Result<(), Error> {
        let cutoff = now() - RETENTION;
        let old_responses: Vec<_> = self
            .record
            .responses
            .iter()
            .filter(|(_, meter)| meter.recorded_at < cutoff)
            .map(|(id, _)| id.clone())
            .collect();
        let old_keys: Vec<_> = self
            .record
            .keys
            .iter()
            .filter(|(_, key)| key.expires_at < cutoff)
            .map(|(id, _)| id.clone())
            .collect();
        let changed = !old_responses.is_empty() || !old_keys.is_empty();
        for id in old_responses {
            if let Some(meter) = self.record.responses.remove(&id) {
                self.record.archived_usage = self
                    .record
                    .archived_usage
                    .checked_add(meter.usage)
                    .map_err(|_| Error::Usage)?;
                if let Some(key) = self.record.keys.get_mut(&meter.key) {
                    key.archived_usage = key
                        .archived_usage
                        .checked_add(meter.usage)
                        .map_err(|_| Error::Usage)?;
                }
            }
        }
        for id in old_keys {
            remove(&self.store.root.join("keys").join(&id))?;
            remove(&self.store.root.join("keys").join(format!("{id}.retired")))?;
            self.record.keys.remove(&id);
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    fn recover_commit(&self) -> Result<(), Error> {
        let journal = self.store.user_path(&self.user, "commit");
        let bytes = match fs::read(&journal) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(Error::Io(error)),
        };
        let (revision, sealed) = bytes.split_first_chunk::<8>().ok_or(Error::Configuration)?;
        let revision = u64::from_be_bytes(*revision);
        let user = self.user.to_string();
        let clear = self.store.cipher.decrypt(
            Aad {
                table: "gateway",
                column: "record",
                row_id: &user,
            },
            sealed,
        )?;
        let committed: Record = serde_json::from_str(clear.expose_secret())?;
        if committed.revision != revision {
            return Err(Error::Configuration);
        }
        let witness = self.store.user_path(&self.user, "witness");
        let receipt: Option<(u64, String)> = fs::read(&witness)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if receipt.as_ref().is_none_or(|r| r.0 <= revision) {
            atomic(&self.store.record_path(&self.user), sealed)?;
            atomic(&witness, &serde_json::to_vec(&(revision, digest(sealed)))?)?;
        }
        remove(&journal)
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

fn remove(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => File::open(path.parent().ok_or(Error::Configuration)?)?
            .sync_all()
            .map_err(Error::Io),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}
