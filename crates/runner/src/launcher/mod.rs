//! Fixed-template Docker control; no worker files or model output are parsed here.
mod client;
mod serve;
mod template;

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use bollard::{Docker, models::ContainerSummary, query_parameters as q};
use contracts::{job::access::McpHttpEndpoint, launcher::WorkerStatus, runtime::ContainerId};
use futures::StreamExt;

pub use client::Client;
pub use serve::serve;
pub use template::Settings;

/// Launcher failures contain no request payloads or credentials.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Operator configuration is incomplete.
    #[error("missing or invalid launcher setting {0}")]
    Configuration(String),
    /// A request or template violates the fixed shape.
    #[error("invalid container identity, image pin or security template")]
    InvalidTemplate,
    /// The engine rejected a control operation.
    #[error("container engine operation failed")]
    Docker(#[from] bollard::errors::Error),
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
}

/// Container state is serialized under the service's lifecycle mutex.
struct Launcher {
    docker: Docker,
    settings: Settings,
    used: HashMap<String, Instant>,
    attached: HashSet<String>,
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
        Ok(self
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
            .collect())
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

    async fn ensure(
        &mut self,
        id: &ContainerId,
        memcan: Option<&McpHttpEndpoint>,
    ) -> Result<(), Error> {
        let config = self.settings.template(id, false, memcan)?;
        let name = self.settings.name(id, false);
        let existing = self.inspect(id, false).await?;
        if existing
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|c| c.image.as_ref())
            .is_some_and(|image| Some(image) != config.image.as_ref())
        {
            return Err(Error::InvalidTemplate);
        }
        let running = self.list().await?;
        if !running.iter().any(|w| &w.container == id && w.running) {
            if self.attached.contains(&name) {
                return Err(Error::Busy);
            }
            let mut candidates: Vec<_> = running.iter().filter(|w| w.running).collect();
            if candidates.len() >= self.settings.max_running {
                candidates.retain(|w| {
                    !self
                        .attached
                        .contains(&self.settings.name(&w.container, false))
                });
                candidates.sort_by_key(|w| {
                    self.used
                        .get(&self.settings.name(&w.container, false))
                        .copied()
                });
                self.stop(&candidates.first().ok_or(Error::Busy)?.container)
                    .await?;
            }
            if existing.is_none() {
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
            if matches!(id, ContainerId::Pr { .. }) && self.fetch(id).await.is_err() {
                tracing::warn!(worker = %name, "dependency sources unavailable after worker start");
            }
        }
        self.used.insert(name, Instant::now());
        Ok(())
    }

    async fn fetch(&self, id: &ContainerId) -> Result<(), Error> {
        let config = self.settings.template(id, true, None)?;
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
                Some(Ok(status)) if status.status_code == 0 => Ok(()),
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

    // TODO(W4): revoke virtual keys before releasing job attachments so idle/LRU eviction has no live key.
    async fn sweep(&mut self) -> Result<(), Error> {
        for worker in self.list().await?.into_iter().filter(|w| w.running) {
            let name = self.settings.name(&worker.container, false);
            if !self.attached.contains(&name)
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
