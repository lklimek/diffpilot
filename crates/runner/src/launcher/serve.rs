use super::{Error, Launcher, Settings};
use bollard::{Docker, container::LogOutput, exec::StartExecResults, models::ExecConfig};
use contracts::launcher::{LauncherAction, LauncherRequest, LauncherResponse};
use futures::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::JoinSet,
};

const MAX_REQUEST: usize = 16 * 1024;

pub(super) async fn read_message<T: DeserializeOwned>(socket: &mut UnixStream) -> Result<T, Error> {
    read_limited(socket, 8 * 1024 * 1024).await
}

pub(super) async fn read_limited<T: DeserializeOwned>(
    socket: &mut UnixStream,
    limit: usize,
) -> Result<T, Error> {
    let mut bytes = Vec::new();
    loop {
        let byte = socket.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        if bytes.len() >= limit {
            return Err(Error::Protocol);
        }
        bytes.push(byte);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}

pub(super) async fn write_message<T: Serialize>(
    socket: &mut UnixStream,
    value: &T,
) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Protocol)?;
    bytes.push(b'\n');
    socket.write_all(&bytes).await?;
    Ok(())
}

pub(super) async fn connection(
    mut socket: UnixStream,
    launcher: Arc<Launcher>,
) -> Result<(), Error> {
    let request: LauncherRequest = tokio::time::timeout(
        Duration::from_secs(10),
        read_limited(&mut socket, MAX_REQUEST),
    )
    .await
    .map_err(|_| Error::Protocol)??;
    let id = request.id.clone();
    let mut upgraded = false;
    let result = handle(&mut socket, request, launcher, &mut upgraded).await;
    if let Err(error) = &result
        && !upgraded
    {
        use contracts::launcher::LauncherFailure;
        let failure = match error {
            Error::Busy => LauncherFailure::Busy,
            Error::InvalidTemplate => LauncherFailure::InvalidTemplate,
            Error::Fetch => LauncherFailure::FetchUnavailable,
            _ => LauncherFailure::WorkerUnavailable,
        };
        write_message(
            &mut socket,
            &LauncherResponse {
                id,
                workers: vec![],
                fetch: None,
                error: Some(failure),
            },
        )
        .await?;
    }
    result
}

async fn handle(
    socket: &mut UnixStream,
    request: LauncherRequest,
    launcher: Arc<Launcher>,
    upgraded: &mut bool,
) -> Result<(), Error> {
    let operation = match &request.action {
        LauncherAction::Ensure { container, .. }
        | LauncherAction::Stop { container }
        | LauncherAction::Remove { container }
        | LauncherAction::Fetch { container }
        | LauncherAction::Attach { container } => {
            let name = launcher.settings.name(container, false);
            Some(launcher.operation(&name).lock_owned().await)
        }
        LauncherAction::List => None,
    };
    // A disconnected control request may have waited behind setup for the same worker.
    match socket.try_read(&mut [0]) {
        Ok(_) => return Err(Error::Protocol),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(error.into()),
    }
    let mut response = LauncherResponse {
        id: request.id,
        error: None,
        fetch: None,
        workers: vec![],
    };
    let fetch = match request.action {
        LauncherAction::Ensure { container, memcan } => launcher
            .ensure(&container, memcan.as_ref())
            .await?
            .then_some((container, true)),
        LauncherAction::Stop { container } => {
            launcher.stop(&container).await?;
            None
        }
        LauncherAction::Remove { container } => {
            launcher.remove(&container).await?;
            None
        }
        LauncherAction::Fetch { container } => Some((container, false)),
        LauncherAction::List => {
            response.workers = launcher.list().await?;
            None
        }
        LauncherAction::Attach { container } => {
            let name = launcher.settings.name(&container, false);
            let running = launcher
                .inspect(&container, false)
                .await?
                .and_then(|worker| worker.state)
                .and_then(|state| state.running)
                == Some(true);
            if !running {
                return Err(Error::Busy);
            }
            let relay = launcher.begin_attachment(&name).ok_or(Error::Busy)?;
            let docker = &launcher.docker;
            let result = async {
                let exec = docker.create_exec(&name, ExecConfig {
                    attach_stdin: Some(true), attach_stdout: Some(true), attach_stderr: Some(true), tty: Some(false),
                    cmd: Some(vec!["codex".into(), "app-server".into(), "proxy".into(), "--sock".into(), "/run/dp/app.sock".into()]), ..Default::default()
                }).await?;
                // The same container guard fences Docker proxy startup against Stop.
                let StartExecResults::Attached { mut output, mut input } = docker.start_exec(&exec.id, None).await?
                    else { return Err(Error::Protocol); };
                drop(operation);
                write_message(socket, &response).await?;
                *upgraded = true;
                let (mut reader, mut writer) = socket.split();
                let incoming = tokio::io::copy(&mut reader, &mut input);
                let outgoing = async {
                    let mut stderr_bytes = 0usize;
                    while let Some(frame) = output.next().await {
                        match frame? {
                            LogOutput::StdOut { message } => writer.write_all(&message).await?,
                            LogOutput::StdErr { message } => stderr_bytes = stderr_bytes.saturating_add(message.len()),
                            _ => return Err(Error::Protocol),
                        }
                    }
                    tracing::debug!(bytes = stderr_bytes, "worker proxy diagnostics drained");
                    Ok::<(), Error>(())
                };
                tokio::select! { result = incoming => { result?; }, result = outgoing => { result?; } }
                Ok::<(), Error>(())
            }.await;
            // W4 stops an interrupted turn explicitly; clean detach begins the idle interval.
            launcher.finish_attachment(name, &relay);
            return result;
        }
    };
    if let Some((container, automatic)) = fetch {
        let name = launcher.settings.name(&container, false);
        if !launcher.state().fetching.insert(name.clone()) {
            return Err(Error::Busy);
        }
        drop(operation);
        let result = launcher.fetch(&container).await;
        launcher.state().fetching.remove(&name);
        response.fetch = Some(
            result
                .as_ref()
                .copied()
                .unwrap_or(contracts::launcher::DependencySources::Unavailable),
        );
        if automatic {
            if result.is_err() {
                tracing::warn!(worker = %name, "dependency sources unavailable after worker start");
            }
        } else {
            result?;
        }
    }
    write_message(socket, &response).await?;
    super::client::close(socket).await
}

pub(super) async fn housekeeping(launcher: Arc<Launcher>, period: Duration) {
    let mut interval = tokio::time::interval(period);
    loop {
        interval.tick().await;
        if let Err(error) = launcher.sweep().await {
            tracing::warn!(%error, "worker idle sweep failed; retrying on next interval");
        }
    }
}

/// Serve until SIGTERM/Ctrl+C; drain connections and stop managed workers before returning.
/// The parent directory must be private to the runner and launcher and writable by their common uid.
pub async fn serve(path: &Path, settings: Settings) -> Result<(), Error> {
    settings.validate()?;
    let docker = Docker::connect_with_socket_defaults()?;
    super::validate_runtime(&docker, &settings).await?;
    let launcher = Launcher::new(docker, settings);
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(Error::Busy);
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    if let Err(error) = launcher.reconcile().await {
        std::fs::remove_file(path)?;
        return Err(error);
    }
    let permits = Arc::new(Semaphore::new(
        launcher.settings.max_running * 2 + crate::config::CONTROL_CONNECTIONS,
    ));
    let shared = Arc::new(launcher);
    let mut tasks = JoinSet::new();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let sweep = tokio::spawn(housekeeping(Arc::clone(&shared), Duration::from_secs(30)));
    let mut failure = None;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = term.recv() => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if !matches!(result, Ok(Ok(()))) { tracing::debug!("launcher connection ended without acknowledgement"); }
            }
            accepted = listener.accept(), if permits.available_permits() > 0 => {
                let (socket, _) = match accepted { Ok(pair) => pair, Err(error) => { failure = Some(error.into()); break; } };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue; };
                let state = Arc::clone(&shared);
                tasks.spawn(async move { let _permit = permit; connection(socket, state).await });
            }
        }
    }
    sweep.abort();
    let _ = sweep.await;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    shared.reconcile().await?;
    std::fs::remove_file(path)?;
    failure.map_or(Ok(()), Err)
}
