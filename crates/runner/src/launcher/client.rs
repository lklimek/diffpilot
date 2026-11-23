use super::{
    Error,
    serve::{read_message, write_message},
};
use contracts::{
    launcher::{LauncherAction, LauncherRequest, LauncherResponse},
    runtime::ContainerId,
};
use std::path::{Path, PathBuf};
use tokio::{io::AsyncWriteExt, net::UnixStream};

/// Private-socket client; the app-server provider owns WebSocket framing after attach.
#[derive(Clone)]
pub struct Client {
    path: PathBuf,
}

impl Client {
    /// Select the operator-owned socket path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_owned(),
        }
    }

    /// Execute one bounded control request.
    pub async fn call(&self, request: LauncherRequest) -> Result<LauncherResponse, Error> {
        let (_, response) = self.connect(request).await?;
        Ok(response)
    }

    async fn connect(
        &self,
        request: LauncherRequest,
    ) -> Result<(UnixStream, LauncherResponse), Error> {
        tokio::time::timeout(
            std::time::Duration::from_secs(330),
            self.connect_inner(request),
        )
        .await
        .map_err(|_| Error::Protocol)?
    }

    async fn connect_inner(
        &self,
        request: LauncherRequest,
    ) -> Result<(UnixStream, LauncherResponse), Error> {
        let mut socket = UnixStream::connect(&self.path).await?;
        write_message(&mut socket, &request).await?;
        let response: LauncherResponse = read_message(&mut socket).await?;
        if response.id != request.id {
            return Err(Error::Protocol);
        }
        if let Some(error) = response.error {
            return Err(Error::Remote(error));
        }
        Ok((socket, response))
    }

    /// Acknowledgement is consumed before exposing raw proxy bytes to the provider.
    pub async fn attach(
        &self,
        request_id: String,
        container: ContainerId,
    ) -> Result<UnixStream, Error> {
        let (socket, _) = self
            .connect(LauncherRequest {
                id: request_id,
                action: LauncherAction::Attach { container },
            })
            .await?;
        Ok(socket)
    }

    /// Runner startup: stop every worker before accepting a new turn, then remove retired PRs.
    /// Returned identities cover existing workers; the runner also revokes keys for missing workers.
    pub async fn reconcile(&self, retired_prs: &[i64]) -> Result<Vec<ContainerId>, Error> {
        let response = self
            .call(LauncherRequest {
                id: "startup-list".into(),
                action: LauncherAction::List,
            })
            .await?;
        let mut stopped = vec![];
        for worker in response.workers {
            let retired = matches!(worker.container, ContainerId::Pr { pull_request_id, .. } if retired_prs.contains(&pull_request_id));
            let action = if retired {
                LauncherAction::Remove {
                    container: worker.container.clone(),
                }
            } else {
                LauncherAction::Stop {
                    container: worker.container.clone(),
                }
            };
            self.call(LauncherRequest {
                id: "startup-stop".into(),
                action,
            })
            .await?;
            stopped.push(worker.container);
        }
        Ok(stopped)
    }
}

pub(super) async fn close(socket: &mut UnixStream) -> Result<(), Error> {
    socket.shutdown().await?;
    Ok(())
}
