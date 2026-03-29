#![allow(clippy::multiple_crate_versions)]

use mc_plugin_api::codec::admin::{
    AdminNamedCountView, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionsView, AdminStatusView,
    AdminTopologyReloadView, RuntimeReloadMode,
};
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceEndpointView, AdminSurfaceInstanceDeclaration, AdminSurfacePauseView,
    AdminSurfaceResource, AdminSurfaceStatusView,
};
use mc_plugin_sdk_rust::admin_surface::{
    AdminSurfaceHost, RustAdminSurfacePlugin, SdkAdminSurfaceHost,
};
use mc_plugin_sdk_rust::capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use revy_voxel_core::{AdminSurfaceCapability, AdminSurfaceCapabilitySet};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

const MANIFEST: StaticPluginManifest = StaticPluginManifest::admin_surface(
    "admin-console",
    "Console Admin Surface Plugin",
    "console-v1",
);

#[derive(Default)]
pub struct ConsoleAdminSurfacePlugin {
    instances: Arc<Mutex<HashMap<String, ConsoleInstance>>>,
}

struct ConsoleInstance {
    principal_id: String,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    worker: Option<ConsoleWorker>,
}

struct ConsoleWorker {
    stop: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
}

impl RustAdminSurfacePlugin for ConsoleAdminSurfacePlugin {
    fn descriptor(&self) -> mc_plugin_api::codec::admin_surface::AdminSurfaceDescriptor {
        mc_plugin_sdk_rust::admin_surface::admin_surface_descriptor("console-v1")
    }

    fn capability_set(&self) -> AdminSurfaceCapabilitySet {
        capabilities::admin_surface_capabilities(&[AdminSurfaceCapability::RuntimeReload])
    }

    fn declare_instance(
        &self,
        _instance_id: &str,
        _surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceInstanceDeclaration, String> {
        Ok(AdminSurfaceInstanceDeclaration {
            principals: Vec::new(),
            required_process_resources: vec!["stdio.stdin".to_string(), "stdio.stdout".to_string()],
            supports_upgrade_handoff: false,
        })
    }

    fn start(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        _surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceStatusView, String> {
        let stdin_fd = take_fd_resource(&host, "stdio.stdin")?;
        let stdout_fd = take_fd_resource(&host, "stdio.stdout")?;
        let principal_id = console_principal_id(instance_id);
        let worker = start_worker(
            instance_id.to_string(),
            principal_id.clone(),
            stdin_fd,
            stdout_fd,
            host,
        )?;
        let mut instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        if let Some(previous) = instances.insert(
            instance_id.to_string(),
            ConsoleInstance {
                principal_id,
                stdin_fd,
                stdout_fd,
                worker: Some(worker),
            },
        ) {
            stop_worker(previous.worker);
            close_fd(previous.stdin_fd);
            close_fd(previous.stdout_fd);
        }
        Ok(console_status())
    }

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        _host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfacePauseView, String> {
        let instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        if !instances.contains_key(instance_id) {
            return Err(format!("console instance `{instance_id}` is not active"));
        }
        Ok(AdminSurfacePauseView {
            resume_payload: Vec::new(),
        })
    }

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        _surface_config_path: Option<&str>,
        _resume_payload: &[u8],
    ) -> Result<AdminSurfaceStatusView, String> {
        let mut instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        if let Some(instance) = instances.get_mut(instance_id) {
            if instance.worker.is_none() {
                instance.worker = Some(start_worker(
                    instance_id.to_string(),
                    instance.principal_id.clone(),
                    instance.stdin_fd,
                    instance.stdout_fd,
                    host,
                )?);
            }
            return Ok(console_status());
        }

        let stdin_fd = take_fd_resource(&host, "stdio.stdin")?;
        let stdout_fd = take_fd_resource(&host, "stdio.stdout")?;
        let principal_id = console_principal_id(instance_id);
        instances.insert(
            instance_id.to_string(),
            ConsoleInstance {
                principal_id: principal_id.clone(),
                stdin_fd,
                stdout_fd,
                worker: None,
            },
        );
        Ok(console_status())
    }

    fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
    ) -> Result<(), String> {
        let mut instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        let instance = instances
            .get_mut(instance_id)
            .ok_or_else(|| format!("console instance `{instance_id}` is not active"))?;
        if instance.worker.is_none() {
            instance.worker = Some(start_worker(
                instance_id.to_string(),
                instance.principal_id.clone(),
                instance.stdin_fd,
                instance.stdout_fd,
                host,
            )?);
        }
        Ok(())
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfaceStatusView, String> {
        let mut instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        let Some(instance) = instances.get_mut(instance_id) else {
            return Ok(console_status());
        };
        if instance.worker.is_none() {
            instance.worker = Some(start_worker(
                instance_id.to_string(),
                instance.principal_id.clone(),
                instance.stdin_fd,
                instance.stdout_fd,
                host,
            )?);
        }
        Ok(console_status())
    }

    fn shutdown(&self, instance_id: &str, _host: SdkAdminSurfaceHost) -> Result<(), String> {
        let mut instances = self
            .instances
            .lock()
            .expect("console admin surface mutex should not be poisoned");
        if let Some(instance) = instances.remove(instance_id) {
            match instance.worker {
                Some(worker) => detach_worker(Some(worker)),
                None => {
                    close_fd(instance.stdin_fd);
                    close_fd(instance.stdout_fd);
                }
            }
        }
        Ok(())
    }
}

fn console_principal_id(instance_id: &str) -> String {
    format!("console:{instance_id}")
}

fn console_status() -> AdminSurfaceStatusView {
    AdminSurfaceStatusView {
        endpoints: vec![AdminSurfaceEndpointView {
            surface: "console".to_string(),
            local_addr: "stdio".to_string(),
        }],
    }
}

fn start_worker(
    instance_id: String,
    principal_id: String,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    host: SdkAdminSurfaceHost,
) -> Result<ConsoleWorker, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let stdin_dup = dup_fd(stdin_fd)?;
    let stdout_dup = dup_fd(stdout_fd)?;
    let join = thread::Builder::new()
        .name(format!("console-admin-surface-{instance_id}"))
        .spawn(move || {
            let stdin = unsafe { std::fs::File::from_raw_fd(stdin_dup) };
            let mut stdout = unsafe { std::fs::File::from_raw_fd(stdout_dup) };
            if let Err(error) = run_console_loop(
                &host,
                &principal_id,
                stdin.as_raw_fd(),
                &mut stdout,
                stop_for_thread,
            ) {
                let _ = writeln!(stdout, "error: {error}");
                let _ = stdout.flush();
            }
        })
        .map_err(|error| format!("failed to spawn console thread: {error}"))?;
    Ok(ConsoleWorker { stop, join })
}

fn stop_worker(worker: Option<ConsoleWorker>) {
    let Some(worker) = worker else {
        return;
    };
    worker.stop.store(true, Ordering::SeqCst);
    let _ = worker.join.join();
}

fn detach_worker(worker: Option<ConsoleWorker>) {
    let Some(worker) = worker else {
        return;
    };
    worker.stop.store(true, Ordering::SeqCst);
    drop(worker);
}

fn run_console_loop(
    host: &SdkAdminSurfaceHost,
    principal_id: &str,
    stdin_fd: RawFd,
    stdout: &mut std::fs::File,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 4096];
    while !stop.load(Ordering::SeqCst) {
        match poll_fd(stdin_fd, 200)? {
            PollResult::Ready => {
                let read = read_fd(stdin_fd, &mut chunk)?;
                if read == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..read]);
                while let Some(line) = take_line(&mut pending)? {
                    handle_line(host, principal_id, stdout, &line)?;
                }
            }
            PollResult::TimedOut => {}
        }
    }
    Ok(())
}

fn handle_line(
    host: &SdkAdminSurfaceHost,
    principal_id: &str,
    stdout: &mut std::fs::File,
    line: &str,
) -> Result<(), String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let request = parse_line(line)?;
    let response = host.execute(principal_id, &request)?;
    let rendered = render_response(&response);
    writeln!(stdout, "{rendered}").map_err(|error| format!("failed to write stdout: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;
    Ok(())
}

fn parse_line(line: &str) -> Result<AdminRequest, String> {
    let trimmed = line.trim();
    let normalized = trimmed
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    match normalized.as_str() {
        "help" => Ok(AdminRequest::Help),
        "status" => Ok(AdminRequest::Status),
        "sessions" => Ok(AdminRequest::Sessions),
        "reload runtime artifacts" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Artifacts,
        }),
        "reload runtime topology" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Topology,
        }),
        "reload runtime core" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Core,
        }),
        "reload runtime full" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Full,
        }),
        _ if normalized.starts_with("upgrade runtime executable ") => {
            let executable_path = trimmed["upgrade runtime executable ".len()..].trim();
            if executable_path.is_empty() {
                Err(format!("unknown command `{line}`; try `help`"))
            } else {
                Ok(AdminRequest::UpgradeRuntime {
                    executable_path: executable_path.to_string(),
                })
            }
        }
        "shutdown" => Ok(AdminRequest::Shutdown),
        _ => Err(format!("unknown command `{line}`; try `help`")),
    }
}

fn render_response(response: &AdminResponse) -> String {
    match response {
        AdminResponse::Help => render_help(),
        AdminResponse::Status(status) => render_status(status),
        AdminResponse::Sessions(sessions) => render_sessions(sessions),
        AdminResponse::ReloadRuntime(result) => render_runtime_reload(result),
        AdminResponse::UpgradeRuntime(result) => {
            format!("upgrade runtime: executable={}", result.executable_path)
        }
        AdminResponse::ShutdownScheduled => "shutdown: scheduled".to_string(),
        AdminResponse::PermissionDenied {
            principal_id,
            permission,
        } => format!(
            "permission denied: principal={} permission={}",
            principal_id,
            permission.as_str()
        ),
        AdminResponse::Error { message } => format!("error: {message}"),
    }
}

fn render_help() -> String {
    [
        "help",
        "status",
        "sessions",
        "reload runtime artifacts",
        "reload runtime topology",
        "reload runtime core",
        "reload runtime full",
        "upgrade runtime executable <path>",
        "shutdown",
    ]
    .join("\n")
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

fn format_named_counts(values: &[AdminNamedCountView]) -> String {
    if values.is_empty() {
        return "-".to_string();
    }
    values
        .iter()
        .map(|entry| format!("{}={}", entry.value.as_deref().unwrap_or("-"), entry.count))
        .collect::<Vec<_>>()
        .join(",")
}

fn render_summary(summary: &AdminSessionSummaryView) -> String {
    format!(
        "sessions={} transport={} phase={} generation={} adapter={} gameplay={}",
        summary.total,
        summary
            .by_transport
            .iter()
            .map(|entry| format!("{:?}={}", entry.transport, entry.count))
            .collect::<Vec<_>>()
            .join(","),
        summary
            .by_phase
            .iter()
            .map(|entry| format!("{:?}={}", entry.phase, entry.count))
            .collect::<Vec<_>>()
            .join(","),
        summary
            .by_generation
            .iter()
            .map(|entry| format!("{}={}", entry.generation_id, entry.count))
            .collect::<Vec<_>>()
            .join(","),
        format_named_counts(&summary.by_adapter_id),
        format_named_counts(&summary.by_gameplay_profile),
    )
}

fn render_status(status: &AdminStatusView) -> String {
    let mut lines = vec![
        format!(
            "runtime active-generation={} draining-generations={} listeners={} sessions={} dirty={}",
            status.active_generation_id,
            status.draining_generation_ids.len(),
            status.listener_bindings.len(),
            status.session_summary.total,
            status.dirty,
        ),
        format!(
            "topology tcp-default={} tcp-enabled={} udp-default={} udp-enabled={} max-players={} motd={:?}",
            status.default_adapter_id,
            join_or_dash(&status.enabled_adapter_ids),
            status.default_bedrock_adapter_id.as_deref().unwrap_or("-"),
            join_or_dash(&status.enabled_bedrock_adapter_ids),
            status.max_players,
            status.motd,
        ),
        render_summary(&status.session_summary),
    ];
    if let Some(plugin_host) = &status.plugin_host {
        lines.push(format!(
            "plugins protocol={} gameplay={} storage={} auth={} admin-surface={} active-quarantines={} artifact-quarantines={} pending-fatal={}",
            plugin_host.protocol_count,
            plugin_host.gameplay_count,
            plugin_host.storage_count,
            plugin_host.auth_count,
            plugin_host.admin_surface_count,
            plugin_host.active_quarantine_count,
            plugin_host.artifact_quarantine_count,
            plugin_host.pending_fatal_error.as_deref().unwrap_or("none"),
        ));
    }
    lines.join("\n")
}

fn render_sessions(sessions: &AdminSessionsView) -> String {
    let mut lines = vec![render_summary(&sessions.summary)];
    if sessions.sessions.is_empty() {
        lines.push("no sessions".to_string());
    } else {
        for session in &sessions.sessions {
            lines.push(format!(
                "conn={} gen={} transport={:?} phase={:?} adapter={} gameplay={} player={} entity={} proto-gen={} gameplay-gen={}",
                session.connection_id.0,
                session.generation_id,
                session.transport,
                session.phase,
                session.adapter_id.as_deref().unwrap_or("-"),
                session.gameplay_profile.as_deref().unwrap_or("-"),
                session
                    .player_id
                    .map(|player_id| player_id.0.hyphenated().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                session
                    .entity_id
                    .map(|entity_id| entity_id.0.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                session
                    .protocol_generation
                    .map(|generation| generation.0.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                session
                    .gameplay_generation
                    .map(|generation| generation.0.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
    }
    lines.join("\n")
}

fn render_reload_topology(result: &AdminTopologyReloadView, mode: RuntimeReloadMode) -> String {
    format!(
        "reload runtime {}: active={} retired={} applied-config-change={} reconfigured={}",
        mode.as_str(),
        result.activated_generation_id,
        if result.retired_generation_ids.is_empty() {
            "-".to_string()
        } else {
            result
                .retired_generation_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        },
        result.applied_config_change,
        if result.reconfigured_adapter_ids.is_empty() {
            "-".to_string()
        } else {
            result.reconfigured_adapter_ids.join(",")
        },
    )
}

fn render_runtime_reload(result: &AdminRuntimeReloadView) -> String {
    match &result.detail {
        AdminRuntimeReloadDetail::Artifacts(detail) => {
            if detail.reloaded_plugin_ids.is_empty() {
                format!(
                    "reload runtime {}: no plugin artifacts changed",
                    result.mode.as_str()
                )
            } else {
                format!(
                    "reload runtime {}: {}",
                    result.mode.as_str(),
                    detail.reloaded_plugin_ids.join(",")
                )
            }
        }
        AdminRuntimeReloadDetail::Topology(detail) => render_reload_topology(detail, result.mode),
        AdminRuntimeReloadDetail::Core(_) => {
            format!("reload runtime {}: completed", result.mode.as_str())
        }
        AdminRuntimeReloadDetail::Full(detail) => {
            let plugins = if detail.reloaded_plugin_ids.is_empty() {
                "-".to_string()
            } else {
                detail.reloaded_plugin_ids.join(",")
            };
            format!(
                "reload runtime {}: plugins={} active={} reconfigured={}",
                result.mode.as_str(),
                plugins,
                detail.topology.activated_generation_id,
                if detail.topology.reconfigured_adapter_ids.is_empty() {
                    "-".to_string()
                } else {
                    detail.topology.reconfigured_adapter_ids.join(",")
                }
            )
        }
    }
}

fn take_line(buffer: &mut Vec<u8>) -> Result<Option<String>, String> {
    let Some(newline_index) = buffer.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let mut line = buffer.drain(..=newline_index).collect::<Vec<_>>();
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        let _ = line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| "console input was not valid utf-8".to_string())
}

fn take_fd_resource(host: &SdkAdminSurfaceHost, name: &str) -> Result<RawFd, String> {
    match host.take_process_resource(name)? {
        Some(AdminSurfaceResource::NativeHandle {
            handle_kind,
            raw_handle,
        }) if handle_kind == "fd" => i32::try_from(raw_handle)
            .map_err(|_| format!("admin surface resource `{name}` did not fit in a raw fd")),
        Some(other) => Err(format!(
            "admin surface resource `{name}` had unexpected shape: {other:?}"
        )),
        None => Err(format!(
            "required admin surface resource `{name}` was not available"
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PollResult {
    Ready,
    TimedOut,
}

fn dup_fd(fd: RawFd) -> Result<RawFd, String> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(duplicated)
    }
}

fn close_fd(fd: RawFd) {
    let _ = unsafe { libc::close(fd) };
}

fn poll_fd(fd: RawFd, timeout_ms: i32) -> Result<PollResult, String> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if ready < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if ready == 0 {
        Ok(PollResult::TimedOut)
    } else {
        Ok(PollResult::Ready)
    }
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> Result<usize, String> {
    let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if read < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(error.to_string());
    }
    usize::try_from(read).map_err(|_| "console read length overflowed usize".to_string())
}

export_plugin!(admin_surface, ConsoleAdminSurfacePlugin, MANIFEST);
