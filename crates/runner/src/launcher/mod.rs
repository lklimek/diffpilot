//! Fixed-template Docker control; no worker files or model output are parsed here.
mod client;
mod serve;
mod template;

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use bollard::{
    Docker,
    exec::StartExecOptions,
    models::{ContainerSummary, ExecConfig, VolumeCreateRequest},
    query_parameters as q,
};
use contracts::{job::access::McpHttpEndpoint, launcher::WorkerStatus, runtime::ContainerId};
use futures::StreamExt;

pub use client::Client;
pub use serve::serve;
pub use template::Settings;

/// Launcher failures contain no request payloads or credentials.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Correlated failure from a remote launcher; never includes engine diagnostics.
    #[error("launcher operation failed: {0:?}")]
    Remote(contracts::launcher::LauncherFailure),
    /// Operator configuration is incomplete.
    #[error("missing or invalid launcher setting {0}")]
    Configuration(String),
    /// A request or template violates the fixed shape.
    #[error("invalid container identity, image pin or security template")]
    InvalidTemplate,
    /// The engine rejected a control operation.
    #[error("container engine operation failed")]
    Docker(#[from] bollard::errors::Error),
    /// Runtime isolation cannot be established without inspecting the engine and networks.
    #[error(
        "cannot inspect {resource}; Docker Engine 28.0+ and existing isolated worker/judge bridges are required"
    )]
    RuntimeInspection {
        /// Engine version or configured network being inspected.
        resource: String,
        /// Underlying engine failure, retained for operator diagnostics.
        #[source]
        source: bollard::errors::Error,
    },
    /// Private socket transport failure.
    #[error("launcher socket operation failed")]
    Io(#[from] std::io::Error),
    /// Malformed or oversized framing.
    #[error("invalid launcher message")]
    Protocol,
    /// No idle worker can be evicted.
    #[error("all worker slots have active attachments")]
    Busy,
    /// Dependency fetching failed; caller continues without dependency sources.
    #[error("dependency fetch failed or exceeded its deadline")]
    Fetch,
    /// Worker could not prepare the isolated job configuration.
    #[error("worker runtime configuration failed or exceeded its deadline")]
    RuntimeConfig,
}

/// Container state is serialized under the service's lifecycle mutex.
#[derive(Clone)]
struct Launcher {
    docker: Docker,
    settings: Settings,
    used: HashMap<String, Instant>,
    attached: HashSet<String>,
    fetching: HashSet<String>,
}

async fn validate_runtime(docker: &Docker, settings: &Settings) -> Result<(), Error> {
    let version = docker
        .version()
        .await
        .map_err(|source| Error::RuntimeInspection {
            resource: "Docker Engine version".into(),
            source,
        })?
        .version
        .unwrap_or_default();
    let mut parts = version.splitn(3, '.');
    let major = parts.next().and_then(|v| v.parse::<u32>().ok());
    let minor = parts.next().and_then(|v| v.parse::<u32>().ok());
    let patch = parts
        .next()
        .and_then(|v| v.split(['-', '+']).next()?.parse::<u32>().ok());
    if !matches!(major, Some(28..)) || minor.is_none() || patch.is_none() {
        return Err(Error::Configuration(
            "Docker Engine 28.0 or newer is required for isolated worker bridges".into(),
        ));
    }
    for name in [&settings.worker_network, &settings.judge_network] {
        let network = docker
            .inspect_network(name, None::<q::InspectNetworkOptions>)
            .await
            .map_err(|source| Error::RuntimeInspection {
                resource: format!("network {name}"),
                source,
            })?;
        let options = network.options.unwrap_or_default();
        let isolated = |family| {
            options
                .get(&format!(
                    "com.docker.network.bridge.gateway_mode_ipv{family}"
                ))
                .map(String::as_str)
                == Some("isolated")
        };
        if network.driver.as_deref() != Some("bridge")
            || network.internal != Some(true)
            || !isolated(4)
            || !(network.enable_ipv6 == Some(false)
                || (network.enable_ipv6 == Some(true) && isolated(6)))
        {
            return Err(Error::Configuration(format!(
                "network {name} must be an internal bridge with isolated IPv4 and IPv6 disabled or isolated; recreate it before starting the launcher"
            )));
        }
    }
    Ok(())
}

impl Launcher {
    async fn objects(&self) -> Result<Vec<ContainerSummary>, Error> {
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![format!("dp.managed={}", self.settings.prefix)],
        )]);
        Ok(self
            .docker
            .list_containers(Some(
                q::ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await?)
    }

    async fn list(&self) -> Result<Vec<WorkerStatus>, Error> {
        let mut workers: Vec<_> = self
            .objects()
            .await?
            .into_iter()
            .filter_map(|c| {
                let container = self.settings.identity(c.labels.as_ref()?)?;
                let expected = format!("/{}", self.settings.name(&container, false));
                c.names
                    .as_ref()?
                    .contains(&expected)
                    .then_some(WorkerStatus {
                        container,
                        running: c.state
                            == Some(bollard::models::ContainerSummaryStateEnum::RUNNING),
                    })
            })
            .collect();
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![format!("dp.managed={}", self.settings.prefix)],
        )]);
        let volumes = self
            .docker
            .list_volumes(Some(
                q::ListVolumesOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await?;
        for volume in volumes.volumes.unwrap_or_default() {
            let Some(id @ ContainerId::Pr { .. }) = self.settings.identity(&volume.labels) else {
                continue;
            };
            if volume.name == format!("{}-work", self.settings.name(&id, false))
                && !workers.iter().any(|worker| worker.container == id)
            {
                // Only the runner knows whether a missing container's archive is retired.
                workers.push(WorkerStatus {
                    container: id,
                    running: false,
                });
            }
        }
        Ok(workers)
    }

    async fn inspect(
        &self,
        id: &ContainerId,
        fetch: bool,
    ) -> Result<Option<bollard::models::ContainerInspectResponse>, Error> {
        self.settings.template(id, fetch, None)?;
        match self
            .docker
            .inspect_container(&self.settings.name(id, fetch), None)
            .await
        {
            Ok(c) => {
                let labels = c
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .ok_or(Error::InvalidTemplate)?;
                let shape = if fetch {
                    "cs3"
                } else if matches!(id, ContainerId::Pr { .. }) {
                    "cs1"
                } else {
                    "cs2"
                };
                let pr_matches = match id {
                    ContainerId::Pr {
                        pull_request_id, ..
                    } => labels.get("dp.pr") == Some(&pull_request_id.to_string()),
                    ContainerId::Judge { .. } => !labels.contains_key("dp.pr"),
                };
                if !pr_matches
                    || labels.get("dp.managed") != Some(&self.settings.prefix)
                    || labels.get("dp.user") != Some(&template::payer(id).0)
                    || labels.get("dp.shape").map(String::as_str) != Some(shape)
                {
                    return Err(Error::InvalidTemplate);
                }
                Ok(Some(c))
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn stop(&mut self, id: &ContainerId) -> Result<(), Error> {
        let name = self.settings.name(id, false);
        if let Some(c) = self.inspect(id, false).await? {
            if c.state.and_then(|s| s.running) == Some(true) {
                self.docker
                    .stop_container(
                        &name,
                        Some(q::StopContainerOptionsBuilder::default().t(5).build()),
                    )
                    .await?;
            }
            if matches!(id, ContainerId::Judge { .. }) {
                self.docker.remove_container(&name, None).await?;
            }
        }
        self.used.remove(&name);
        Ok(())
    }

    async fn remove(&mut self, id: &ContainerId) -> Result<(), Error> {
        if self.fetching.contains(&self.settings.name(id, false)) {
            return Err(Error::Busy);
        }
        self.stop(id).await?;
        let name = self.settings.name(id, false);
        if self.inspect(id, false).await?.is_some() {
            self.docker.remove_container(&name, None).await?;
        }
        if matches!(id, ContainerId::Pr { .. }) {
            match self
                .docker
                .remove_volume(&format!("{name}-work"), None::<q::RemoveVolumeOptions>)
                .await
            {
                Ok(())
                | Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    async fn work_volume(&self, id: &ContainerId) -> Result<(), Error> {
        if !matches!(id, ContainerId::Pr { .. }) {
            return Ok(());
        }
        let mut labels = self
            .settings
            .template(id, false, None)?
            .labels
            .ok_or(Error::InvalidTemplate)?;
        labels.remove("dp.template");
        let volume = self
            .docker
            .create_volume(VolumeCreateRequest {
                name: Some(format!("{}-work", self.settings.name(id, false))),
                labels: Some(labels),
                ..Default::default()
            })
            .await?;
        // Existing pre-release volumes have no labels; never discard their thread history.
        if !volume.labels.is_empty() && self.settings.identity(&volume.labels).as_ref() != Some(id)
        {
            return Err(Error::InvalidTemplate);
        }
        Ok(())
    }

    async fn configure(
        &self,
        id: &ContainerId,
        memcan: Option<&McpHttpEndpoint>,
    ) -> Result<(), Error> {
        let shape = if matches!(id, ContainerId::Pr { .. }) {
            "cs1"
        } else {
            "cs2"
        };
        let env = memcan.map(|endpoint| {
            vec![
                format!("DP_MEMCAN_URL={}", endpoint.url),
                format!("DP_MEMCAN_TOKEN={}", endpoint.token.expose()),
            ]
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            // Credentials belong only to this short-lived exec, never the container's env.
            let exec = self
                .docker
                .create_exec(
                    &self.settings.name(id, false),
                    ExecConfig {
                        cmd: Some(vec![
                            "python3".into(),
                            "/usr/local/bin/worker-config.py".into(),
                            shape.into(),
                        ]),
                        env,
                        ..Default::default()
                    },
                )
                .await?;
            self.docker
                .start_exec(
                    &exec.id,
                    Some(StartExecOptions {
                        detach: true,
                        ..Default::default()
                    }),
                )
                .await?;
            loop {
                let state = self.docker.inspect_exec(&exec.id).await?;
                if state.running == Some(false) {
                    return if state.exit_code == Some(0) {
                        Ok(())
                    } else {
                        Err(Error::RuntimeConfig)
                    };
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or(Err(Error::RuntimeConfig))
    }

    // Return whether a CS1 start needs CS3; the service performs it outside the lifecycle lock.
    async fn ensure(
        &mut self,
        id: &ContainerId,
        memcan: Option<&McpHttpEndpoint>,
    ) -> Result<bool, Error> {
        let config = self.settings.template(id, false, memcan)?;
        let name = self.settings.name(id, false);
        if self.attached.contains(&name) || self.fetching.contains(&name) {
            return Err(Error::Busy);
        }
        let mut existing = self.inspect(id, false).await?;
        let drifted = existing.as_ref().is_some_and(|c| {
            c.config.as_ref().is_none_or(|c| {
                c.image != config.image
                    || c.labels.as_ref().and_then(|l| l.get("dp.template"))
                        != config.labels.as_ref().and_then(|l| l.get("dp.template"))
            })
        });
        if drifted {
            if existing
                .as_ref()
                .and_then(|c| c.state.as_ref())
                .and_then(|s| s.running)
                == Some(true)
            {
                self.docker
                    .stop_container(
                        &name,
                        Some(q::StopContainerOptionsBuilder::default().t(5).build()),
                    )
                    .await?;
            }
            self.docker.remove_container(&name, None).await?;
            existing = None;
        }
        let started = existing
            .as_ref()
            .and_then(|c| c.state.as_ref())
            .and_then(|s| s.running)
            != Some(true);
        if started {
            let running = self.list().await?;
            let mut candidates: Vec<_> = running
                .iter()
                .filter(|w| w.running && &w.container != id)
                .collect();
            if candidates.len() >= self.settings.max_running {
                let needed = candidates.len() - self.settings.max_running + 1;
                candidates.retain(|w| {
                    let name = self.settings.name(&w.container, false);
                    !self.attached.contains(&name) && !self.fetching.contains(&name)
                });
                candidates.sort_by_key(|w| {
                    self.used
                        .get(&self.settings.name(&w.container, false))
                        .copied()
                });
                if candidates.len() < needed {
                    return Err(Error::Busy);
                }
                for candidate in candidates.into_iter().take(needed) {
                    self.stop(&candidate.container).await?;
                }
            }
            if existing.is_none() {
                self.work_volume(id).await?;
                self.docker
                    .create_container(
                        Some(
                            q::CreateContainerOptionsBuilder::default()
                                .name(&name)
                                .build(),
                        ),
                        config,
                    )
                    .await?;
            }
            self.docker.start_container(&name, None).await?;
        }
        if let Err(error) = self.configure(id, memcan).await {
            self.stop(id).await?;
            return Err(error);
        }
        self.used.insert(name, Instant::now());
        Ok(started && matches!(id, ContainerId::Pr { .. }))
    }

    async fn fetch(
        &self,
        id: &ContainerId,
    ) -> Result<contracts::launcher::DependencySources, Error> {
        let config = self.settings.template(id, true, None)?;
        self.work_volume(id).await?;
        let name = self.settings.name(id, true);
        if self.inspect(id, true).await?.is_some() {
            self.docker
                .remove_container(
                    &name,
                    Some(
                        q::RemoveContainerOptionsBuilder::default()
                            .force(true)
                            .build(),
                    ),
                )
                .await?;
        }
        self.docker
            .create_container(
                Some(
                    q::CreateContainerOptionsBuilder::default()
                        .name(&name)
                        .build(),
                ),
                config,
            )
            .await?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
            self.docker.start_container(&name, None).await?;
            match self.docker.wait_container(&name, None).next().await {
                Some(Ok(status)) if status.status_code == 0 => {
                    let mut outcome = contracts::launcher::DependencySources::Available;
                    let mut logs = self.docker.logs(
                        &name,
                        Some(q::LogsOptionsBuilder::default().stdout(true).build()),
                    );
                    let mut bytes = 0usize;
                    while let Some(Ok(bollard::container::LogOutput::StdOut { message })) =
                        logs.next().await
                    {
                        bytes = bytes.saturating_add(message.len());
                        if String::from_utf8_lossy(&message)
                            .lines()
                            .any(|line| line == "dependency sources resolved without a lock file")
                        {
                            outcome = contracts::launcher::DependencySources::ResolvedWithoutLock;
                        }
                        if bytes > 1024 * 1024 {
                            break;
                        }
                    }
                    Ok(outcome)
                }
                _ => Err(Error::Fetch),
            }
        })
        .await
        .unwrap_or(Err(Error::Fetch));
        let cleanup = self
            .docker
            .remove_container(
                &name,
                Some(
                    q::RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .build(),
                ),
            )
            .await;
        cleanup?;
        result
    }

    // The provider retires local keys before releasing a job attachment.
    async fn sweep(&mut self) -> Result<(), Error> {
        for worker in self.list().await?.into_iter().filter(|w| w.running) {
            let name = self.settings.name(&worker.container, false);
            if !self.attached.contains(&name)
                && !self.fetching.contains(&name)
                && self
                    .used
                    .get(&name)
                    .is_none_or(|t| t.elapsed().as_secs() >= self.settings.idle_seconds)
            {
                self.stop(&worker.container).await?;
            }
        }
        Ok(())
    }

    async fn reconcile(&mut self) -> Result<(), Error> {
        for worker in self.list().await? {
            self.stop(&worker.container).await?;
        }
        // CS3 may have survived an interrupted launcher; never adopt an unfinished fetch.
        for c in self.objects().await? {
            if c.labels
                .as_ref()
                .and_then(|l| l.get("dp.shape"))
                .map(String::as_str)
                == Some("cs3")
                && let Some(id) = c.id
            {
                self.docker
                    .remove_container(
                        &id,
                        Some(
                            q::RemoveContainerOptionsBuilder::default()
                                .force(true)
                                .build(),
                        ),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
