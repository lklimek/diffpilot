use std::collections::HashMap;

use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountType};
use contracts::{ids::UserId, job::access::McpHttpEndpoint, runtime::ContainerId};

use super::Error;
use crate::config::{WORKER_MAX_RUNNING, max_running, valid_max_running};
use sha2::{Digest, Sha256};

/// Operator-owned limits and image pins; none are accepted from socket requests.
#[derive(Clone)]
pub struct Settings {
    /// IM3 digest.
    pub worker_image: String,
    /// IM4 digest (toolchain 1.100.0).
    pub fetch_image: String,
    /// Namespace for containers, volumes and labels.
    pub prefix: String,
    /// Internal network containing Squid and the model listener.
    pub worker_network: String,
    /// Internal network containing only judges and the model listener.
    pub judge_network: String,
    /// Explicit Squid URL.
    pub proxy: String,
    /// Memory and swap ceiling in bytes.
    pub memory: i64,
    /// CPU ceiling.
    pub cpus: i64,
    /// Process ceiling.
    pub pids: i64,
    /// Running CS1/CS2 ceiling.
    pub max_running: usize,
    /// Idle stop interval in seconds.
    pub idle_seconds: u64,
}

fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(super) fn payer(id: &ContainerId) -> &UserId {
    match id {
        ContainerId::Pr { payer, .. } | ContainerId::Judge { payer } => payer,
    }
}

impl Settings {
    /// Read launcher-only configuration without loading any provider credentials.
    pub fn from_env() -> Result<Self, Error> {
        let get = |key: &str| std::env::var(key).map_err(|_| Error::Configuration(key.into()));
        let number = |key: &str, default: i64| -> Result<i64, Error> {
            match std::env::var(key) {
                Ok(s) => s.parse().map_err(|_| Error::Configuration(key.into())),
                Err(std::env::VarError::NotPresent) => Ok(default),
                Err(_) => Err(Error::Configuration(key.into())),
            }
        };
        let capacity = match std::env::var(WORKER_MAX_RUNNING) {
            Ok(value) => max_running(Some(value.trim())),
            Err(std::env::VarError::NotPresent) => max_running(None),
            Err(_) => None,
        };
        let settings = Self {
            worker_image: get("SR_WORKER_IMAGE")?,
            fetch_image: get("SR_FETCH_IMAGE")?,
            prefix: get("SR_CONTAINER_PREFIX")?,
            worker_network: get("SR_WORKER_NETWORK")?,
            judge_network: get("SR_JUDGE_NETWORK")?,
            proxy: get("SR_WORKER_PROXY")?,
            memory: number("SR_WORKER_MEMORY", 2 * 1024 * 1024 * 1024)?,
            cpus: number("SR_WORKER_CPUS", 2)?,
            pids: number("SR_WORKER_PIDS", 1024)?,
            max_running: capacity
                .ok_or_else(|| Error::Configuration(WORKER_MAX_RUNNING.into()))?
                .get(),
            idle_seconds: number("SR_WORKER_IDLE_STOP", 1200)?
                .try_into()
                .map_err(|_| Error::InvalidTemplate)?,
        };
        settings.validate()?;
        Ok(settings)
    }

    /// Reject unpinned images and invalid resource limits before opening the socket.
    pub fn validate(&self) -> Result<(), Error> {
        let pinned = |s: &str| {
            s.split_once("@sha256:").is_some_and(|(name, hash)| {
                !name.is_empty() && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
            })
        };
        if !component(&self.prefix)
            || !component(&self.worker_network)
            || !component(&self.judge_network)
            || self.worker_network == self.judge_network
            || !pinned(&self.worker_image)
            || !pinned(&self.fetch_image)
            || self.memory <= 0
            || self.cpus <= 0
            || self.cpus > 16
            || self.pids <= 0
            || !valid_max_running(self.max_running)
            || self.idle_seconds == 0
            || !self.proxy.starts_with("http://")
        {
            return Err(Error::InvalidTemplate);
        }
        Ok(())
    }

    /// Stable name derived only from validated identity and operator namespace.
    pub fn name(&self, id: &ContainerId, fetch: bool) -> String {
        match id {
            ContainerId::Pr {
                pull_request_id,
                payer,
            } => format!(
                "{}-{}-{pull_request_id}-{payer}",
                self.prefix,
                if fetch { "fetch" } else { "pr" }
            ),
            ContainerId::Judge { payer } => format!("{}-user-{payer}", self.prefix),
        }
    }

    /// Decode only this launcher's labels; malformed or foreign objects are ignored.
    pub fn identity(&self, labels: &HashMap<String, String>) -> Option<ContainerId> {
        if labels.get("dp.managed")? != &self.prefix {
            return None;
        }
        let payer = UserId(labels.get("dp.user")?.clone());
        if !component(&payer.0) {
            return None;
        }
        match labels.get("dp.shape")?.as_str() {
            "cs1" => {
                let pull_request_id = labels.get("dp.pr")?.parse().ok()?;
                (pull_request_id > 0).then_some(ContainerId::Pr {
                    pull_request_id,
                    payer,
                })
            }
            "cs2" => Some(ContainerId::Judge { payer }),
            _ => None,
        }
    }

    /// Construct CS1, CS2 or its one-shot CS3 from the fixed security template.
    pub fn template(
        &self,
        id: &ContainerId,
        fetch: bool,
        memcan: Option<&McpHttpEndpoint>,
    ) -> Result<ContainerCreateBody, Error> {
        self.validate()?;
        let pr = matches!(id, ContainerId::Pr { pull_request_id, .. } if *pull_request_id > 0);
        if !component(&payer(id).0)
            || matches!(id, ContainerId::Pr { .. }) && !pr
            || !pr && (fetch || memcan.is_some())
        {
            return Err(Error::InvalidTemplate);
        }
        let shape = if fetch {
            "cs3"
        } else if pr {
            "cs1"
        } else {
            "cs2"
        };
        let mut labels = HashMap::from([
            ("dp.managed".into(), self.prefix.clone()),
            ("dp.shape".into(), shape.into()),
            ("dp.user".into(), payer(id).0.clone()),
        ]);
        if let ContainerId::Pr {
            pull_request_id, ..
        } = id
        {
            labels.insert("dp.pr".into(), pull_request_id.to_string());
        }
        let mount = |source: String, target: &str, read_only: bool| Mount {
            typ: Some(MountType::VOLUME),
            source: Some(source),
            target: Some(target.into()),
            read_only: Some(read_only),
            ..Default::default()
        };
        let work = format!("{}-work", self.name(id, false));
        let mut mounts = vec![];
        if pr {
            mounts.push(mount(work, "/work", fetch));
        }
        if fetch {
            mounts.push(mount(format!("{}-c2", self.prefix), "/c2", false));
        } else if pr {
            mounts.push(mount(format!("{}-c1", self.prefix), "/mirror", true));
            mounts.push(mount(format!("{}-c2", self.prefix), "/cargo", true));
        }
        let mut tmpfs = HashMap::from([
            (
                "/tmp".into(),
                "uid=10002,gid=10002,mode=0700,size=2g".into(),
            ),
            (
                "/run/dp".into(),
                "uid=10002,gid=10002,mode=0700,size=1m".into(),
            ),
        ]);
        let mut env = vec!["NO_COLOR=1".into(), "TMPDIR=/tmp".into()];
        if fetch {
            env.extend(
                [
                    "HOME=/tmp",
                    "CARGO_HOME=/c2",
                    "RUSTUP_HOME=/usr/local/rustup",
                    "RUSTUP_AUTO_INSTALL=0",
                    "RUSTUP_TOOLCHAIN=1.100.0-x86_64-unknown-linux-gnu",
                    "CARGO_BUILD_JOBS=2",
                ]
                .map(str::to_owned),
            );
        } else if pr {
            env.extend(
                [
                    "HOME=/work/home",
                    "CODEX_HOME=/work/codex",
                    "CARGO_HOME=/cargo",
                    "CLAUDE_PLUGIN_ROOT=/etc/codex/claudius",
                ]
                .map(str::to_owned),
            );
        } else {
            env.extend(["HOME=/home/worker", "CODEX_HOME=/codex"].map(str::to_owned));
            for path in ["/home/worker", "/codex"] {
                tmpfs.insert(path.into(), "uid=10002,gid=10002,mode=0700,size=64m".into());
            }
        }
        if pr {
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                env.push(format!("{key}={}", self.proxy));
            }
            for key in ["NO_PROXY", "no_proxy"] {
                env.push(format!("{key}=codex-gateway"));
            }
        }
        let mut config = ContainerCreateBody {
            image: Some(
                if fetch {
                    &self.fetch_image
                } else {
                    &self.worker_image
                }
                .clone(),
            ),
            user: Some("10002:10002".into()),
            working_dir: Some(
                if fetch {
                    "/"
                } else if pr {
                    "/work"
                } else {
                    "/tmp"
                }
                .into(),
            ),
            cmd: (!fetch).then(|| vec![shape.into()]),
            labels: Some(labels),
            env: Some(env),
            host_config: Some(HostConfig {
                init: Some(true),
                privileged: Some(false),
                cap_drop: Some(vec!["ALL".into()]),
                security_opt: Some(vec!["no-new-privileges:true".into()]),
                readonly_rootfs: Some(true),
                network_mode: Some(
                    if pr {
                        &self.worker_network
                    } else {
                        &self.judge_network
                    }
                    .clone(),
                ),
                memory: Some(self.memory),
                memory_swap: Some(self.memory),
                nano_cpus: Some(self.cpus * 1_000_000_000),
                pids_limit: Some(self.pids),
                mounts: Some(mounts),
                tmpfs: Some(tmpfs),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Sort recursively even when serde_json's preserve_order feature is unified in.
        let mut value = serde_json::to_value(&config).map_err(|_| Error::Protocol)?;
        value.sort_all_objects();
        let bytes = serde_json::to_vec(&value).map_err(|_| Error::Protocol)?;
        config
            .labels
            .as_mut()
            .ok_or(Error::InvalidTemplate)?
            .insert(
                "dp.template".into(),
                Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            );
        Ok(config)
    }
}
