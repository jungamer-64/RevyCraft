pub(crate) use crate::lock::upgrade_test_lock;
pub(crate) use crate::options::remote_admin_upgrade_options;
pub(crate) use crate::sessions::{JavaPlaySession, StatusSession};
use crate::support::*;
use mc_plugin_admin_grpc::admin as proto;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tonic::metadata::MetadataValue;
use tonic::{Code, Request};

pub(crate) type AdminClient =
    proto::admin_control_plane_client::AdminControlPlaneClient<tonic::transport::Channel>;
pub(crate) type UpgradeResponse =
    Result<tonic::Response<proto::UpgradeRuntimeResponse>, tonic::Status>;
pub(crate) type UpgradeTask = tokio::task::JoinHandle<UpgradeResponse>;

pub(crate) const SERVER_BOOTSTRAP_BIN: &str = env!("CARGO_BIN_EXE_server-bootstrap");

pub(crate) struct PreparedServer {
    temp_dir: TempDir,
    pub(crate) grpc_addr: SocketAddr,
}

pub(crate) fn authorized_request<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {OPS_TOKEN}"))
            .expect("bearer token metadata should be valid"),
    );
    request
}

pub(crate) async fn grpc_client(
    local_addr: SocketAddr,
) -> Result<AdminClient, tonic::transport::Error> {
    proto::admin_control_plane_client::AdminControlPlaneClient::connect(format!(
        "http://{local_addr}"
    ))
    .await
}

#[cfg(unix)]
pub(crate) fn wait_for_output_contains(
    path: &Path,
    needle: &str,
    timeout: Duration,
) -> TestResult<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let content = fs::read_to_string(path).unwrap_or_default();
        if content.contains(needle) {
            return Ok(content);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for `{needle}` in `{}`; current content: {content}",
                path.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn wait_for_tcp_closed(addr: SocketAddr, timeout: Duration) -> TestResult<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
            Ok(stream) => {
                drop(stream);
                if Instant::now() >= deadline {
                    return Err(format!("timed out waiting for {addr} to close").into());
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Ok(()),
        }
    }
}

pub(crate) async fn wait_for_grpc_client(
    local_addr: SocketAddr,
    timeout: Duration,
) -> TestResult<AdminClient> {
    let deadline = Instant::now() + timeout;
    loop {
        match grpc_client(local_addr).await {
            Ok(client) => return Ok(client),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
}

pub(crate) fn runtime_tcp_listener_addr(status: &proto::AdminStatusView) -> TestResult<SocketAddr> {
    let binding = status
        .listener_bindings
        .iter()
        .find(|binding| binding.transport == proto::TransportKind::Tcp as i32)
        .ok_or("status did not include a TCP game listener binding")?;
    Ok(binding.local_addr.parse()?)
}

pub(crate) async fn fetch_runtime_tcp_listener_addr(
    client: &mut AdminClient,
) -> TestResult<SocketAddr> {
    let status = fetch_status(client).await?;
    runtime_tcp_listener_addr(&status)
}

pub(crate) async fn fetch_status(client: &mut AdminClient) -> TestResult<proto::AdminStatusView> {
    Ok(client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .ok_or("status response was missing runtime status")?)
}

pub(crate) async fn wait_for_upgrade_phase(
    client: &mut AdminClient,
    expected_role: proto::RuntimeUpgradeRole,
    expected_phase: proto::RuntimeUpgradePhase,
    timeout: Duration,
) -> TestResult<proto::AdminStatusView> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = fetch_status(client).await?;
        if let Some(upgrade) = status.upgrade.as_ref() {
            let role = proto::RuntimeUpgradeRole::try_from(upgrade.role)
                .map_err(|_| "status upgrade role was invalid")?;
            let phase = proto::RuntimeUpgradePhase::try_from(upgrade.phase)
                .map_err(|_| "status upgrade phase was invalid")?;
            if role == expected_role && phase == expected_phase {
                return Ok(status);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for upgrade phase role={expected_role:?} phase={expected_phase:?}"
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) async fn upgrade_runtime_executable(
    client: &mut AdminClient,
    executable_path: &str,
) -> TestResult<proto::AdminUpgradeRuntimeView> {
    Ok(client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: executable_path.to_string(),
        }))
        .await?
        .into_inner()
        .result
        .ok_or("upgrade response was missing result")?)
}

pub(crate) async fn upgrade_to_current_bootstrap(client: &mut AdminClient) -> TestResult<()> {
    let upgrade = upgrade_runtime_executable(client, SERVER_BOOTSTRAP_BIN).await?;
    assert_eq!(upgrade.executable_path, SERVER_BOOTSTRAP_BIN);
    Ok(())
}

pub(crate) async fn reload_runtime_full(client: &mut AdminClient) -> TestResult<()> {
    let response = client
        .reload_runtime(authorized_request(proto::ReloadRuntimeRequest {
            mode: proto::RuntimeReloadMode::Full as i32,
        }))
        .await?
        .into_inner();
    if response.result.is_none() {
        return Err("reload runtime response was missing result".into());
    }
    Ok(())
}

pub(crate) fn persisted_log_diagnostics(logs: &PersistedServerLogCapture) -> String {
    match logs.read() {
        Ok((stdout, stderr)) => format!("stdout:\n{stdout}\nstderr:\n{stderr}"),
        Err(error) => format!("failed to read persisted server logs: {error}"),
    }
}

pub(crate) fn wait_for_clean_parent_exit(
    parent: &mut Child,
    logs: &PersistedServerLogCapture,
    timeout: Duration,
    expectation: &str,
) -> TestResult<()> {
    let exit_status = wait_for_exit(parent, timeout)?;
    let diagnostics = persisted_log_diagnostics(logs);
    let Some(exit_status) = exit_status else {
        return Err(format!("{expectation}; {diagnostics}").into());
    };
    if !exit_status.success() {
        return Err(format!("{expectation}; status={exit_status}; {diagnostics}").into());
    }
    Ok(())
}

pub(crate) async fn shutdown_runtime_via_grpc(
    client: &mut AdminClient,
    grpc_addr: SocketAddr,
    parent: &mut Child,
    logs: &PersistedServerLogCapture,
) -> TestResult<()> {
    if let Err(error) = client
        .shutdown(authorized_request(proto::ShutdownRequest {}))
        .await
    {
        if !matches!(
            error.code(),
            Code::Unknown | Code::Unavailable | Code::Internal
        ) {
            return Err(Box::new(error));
        }
    }
    wait_for_tcp_closed(grpc_addr, Duration::from_secs(10)).map_err(|error| {
        format!(
            "upgraded child gRPC listener should close after shutdown: {error}; {}",
            persisted_log_diagnostics(logs)
        )
    })?;
    wait_for_clean_parent_exit(
        parent,
        logs,
        Duration::from_secs(10),
        "original parent process should exit cleanly after successful cutover",
    )?;
    Ok(())
}

pub(crate) async fn expect_upgrade_error(
    client: &mut AdminClient,
    executable_path: &str,
    expected_code: Code,
    context: &str,
) -> TestResult<()> {
    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: executable_path.to_string(),
        }))
        .await
        .expect_err(context);
    assert_eq!(error.code(), expected_code);
    Ok(())
}

pub(crate) fn kill_server(child: &mut Child) -> TestResult<()> {
    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

pub(crate) fn write_stdin_lines(child: &mut Child, lines: &[&str]) -> TestResult<()> {
    let stdin = child.stdin.as_mut().ok_or("child stdin should be piped")?;
    for line in lines {
        writeln!(stdin, "{line}")?;
    }
    Ok(())
}

pub(crate) fn assert_process_alive(child: &mut Child, context: &str) -> TestResult<()> {
    if let Some(status) = child.try_wait()? {
        return Err(format!("{context}; status={status}").into());
    }
    Ok(())
}

pub(crate) fn spawn_upgrade_task(mut client: AdminClient) -> UpgradeTask {
    tokio::spawn(async move {
        client
            .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
                executable_path: SERVER_BOOTSTRAP_BIN.to_string(),
            }))
            .await
    })
}

pub(crate) async fn assert_upgrade_task_succeeded(task: UpgradeTask) -> TestResult<()> {
    let upgrade = task.await??.into_inner();
    let result = upgrade
        .result
        .ok_or("upgrade response was missing result")?;
    assert_eq!(result.executable_path, SERVER_BOOTSTRAP_BIN);
    Ok(())
}

pub(crate) async fn assert_upgrade_task_failed(
    task: UpgradeTask,
    expected_code: Code,
    context: &str,
) -> TestResult<()> {
    let error = task.await?.expect_err(context);
    assert_eq!(error.code(), expected_code);
    Ok(())
}

impl PreparedServer {
    pub(crate) fn new<F>(configure: F) -> TestResult<Self>
    where
        F: FnOnce(&Path, u16) -> TestResult<ServerTomlOptions<'static>>,
    {
        let temp_dir = tempdir()?;
        let grpc_port = reserve_port()?;
        let world_dir = temp_dir.path().join("world");
        fs::create_dir_all(&world_dir)?;
        let repo_root = repo_root()?;
        let options = configure(temp_dir.path(), grpc_port)?;
        write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;
        Ok(Self {
            temp_dir,
            grpc_addr: SocketAddr::from(([127, 0, 0, 1], grpc_port)),
        })
    }

    pub(crate) fn remote_admin(motd: &'static str) -> TestResult<Self> {
        Self::new(|_, grpc_port| Ok(remote_admin_upgrade_options(grpc_port, motd)))
    }

    pub(crate) fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    pub(crate) fn missing_bootstrap_path(&self) -> String {
        self.temp_path()
            .join("missing-server-bootstrap")
            .display()
            .to_string()
    }

    pub(crate) async fn wait_for_client(&self, timeout: Duration) -> TestResult<AdminClient> {
        wait_for_grpc_client(self.grpc_addr, timeout).await
    }

    pub(crate) fn spawn_logged(
        &self,
        capture_name: &str,
    ) -> TestResult<(Child, PersistedServerLogCapture)> {
        spawn_server_with_log_capture_and_envs(
            self.temp_path(),
            Stdio::null(),
            None,
            &[],
            capture_name,
        )
    }

    pub(crate) fn spawn_logged_with_envs(
        &self,
        capture_name: &str,
        extra_envs: &[(&str, &str)],
    ) -> TestResult<(Child, PersistedServerLogCapture)> {
        spawn_server_with_log_capture_and_envs(
            self.temp_path(),
            Stdio::null(),
            None,
            extra_envs,
            capture_name,
        )
    }

    pub(crate) fn spawn_piped(&self) -> TestResult<Child> {
        spawn_server(
            self.temp_path(),
            Stdio::piped(),
            Stdio::piped(),
            Stdio::piped(),
        )
    }

    pub(crate) fn spawn_piped_with_envs(&self, extra_envs: &[(&str, &str)]) -> TestResult<Child> {
        spawn_server_with_config_path_and_envs(
            self.temp_path(),
            Stdio::piped(),
            Stdio::piped(),
            Stdio::piped(),
            None,
            extra_envs,
        )
    }

    #[cfg(unix)]
    pub(crate) fn spawn_with_log_files(
        &self,
        stdout_path: &Path,
        stderr_path: &Path,
    ) -> TestResult<Child> {
        let stdout_file = File::create(stdout_path)?;
        let stderr_file = File::create(stderr_path)?;
        spawn_server(
            self.temp_path(),
            Stdio::piped(),
            Stdio::from(stdout_file),
            Stdio::from(stderr_file),
        )
    }
}
