#![allow(clippy::multiple_crate_versions)]
use mc_core::{AdminUiCapability, AdminUiCapabilitySet};
use mc_plugin_api::codec::admin_ui::{
    AdminNamedCountView, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionsView, AdminStatusView,
    AdminTopologyReloadView, AdminUiDescriptor, RuntimeReloadMode,
};
use mc_plugin_sdk_rust::admin_ui::RustAdminUiPlugin;
use mc_plugin_sdk_rust::capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct ConsoleAdminUiPlugin;

impl RustAdminUiPlugin for ConsoleAdminUiPlugin {
    fn descriptor(&self) -> AdminUiDescriptor {
        AdminUiDescriptor {
            ui_profile: "console-v1".into(),
        }
    }

    fn capability_set(&self) -> AdminUiCapabilitySet {
        capabilities::admin_ui_capabilities(&[AdminUiCapability::RuntimeReload])
    }

    fn parse_line(&self, line: &str) -> Result<AdminRequest, String> {
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

    fn render_response(&self, response: &AdminResponse) -> Result<String, String> {
        Ok(match response {
            AdminResponse::Help => render_help(),
            AdminResponse::Status(status) => render_status(status),
            AdminResponse::Sessions(sessions) => render_sessions(sessions),
            AdminResponse::ReloadRuntime(result) => render_runtime_reload(result),
            AdminResponse::UpgradeRuntime(result) => {
                format!(
                    "upgrade runtime: scheduled executable={}",
                    result.executable_path
                )
            }
            AdminResponse::ShutdownScheduled => "shutdown scheduled".to_string(),
            AdminResponse::PermissionDenied {
                principal,
                permission,
            } => format!(
                "permission denied: principal={} permission={}",
                principal.as_str(),
                permission.as_str()
            ),
            AdminResponse::Error { message } => format!("error: {message}"),
        })
    }
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::admin_ui("admin-ui-console", "Console Admin UI Plugin", "console-v1");

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
            "plugins protocol={} gameplay={} storage={} auth={} admin-ui={} active-quarantines={} artifact-quarantines={} pending-fatal={}",
            plugin_host.protocol_count,
            plugin_host.gameplay_count,
            plugin_host.storage_count,
            plugin_host.auth_count,
            plugin_host.admin_ui_count,
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

export_plugin!(admin_ui, ConsoleAdminUiPlugin, MANIFEST);

#[cfg(test)]
mod tests {
    use super::*;
    use mc_plugin_api::codec::admin_ui::{
        AdminPermission, AdminPrincipal, AdminUpgradeRuntimeView,
    };

    #[test]
    fn parses_upgrade_runtime_executable_command() {
        let plugin = ConsoleAdminUiPlugin;
        assert_eq!(
            plugin
                .parse_line("upgrade runtime executable /tmp/server-bootstrap")
                .expect("upgrade command should parse"),
            AdminRequest::UpgradeRuntime {
                executable_path: "/tmp/server-bootstrap".to_string(),
            }
        );
    }

    #[test]
    fn renders_upgrade_runtime_and_permission_denied_responses() {
        let plugin = ConsoleAdminUiPlugin;
        assert_eq!(
            plugin
                .render_response(&AdminResponse::UpgradeRuntime(AdminUpgradeRuntimeView {
                    executable_path: "/tmp/server-bootstrap".to_string(),
                }))
                .expect("upgrade response should render"),
            "upgrade runtime: scheduled executable=/tmp/server-bootstrap"
        );
        assert_eq!(
            plugin
                .render_response(&AdminResponse::PermissionDenied {
                    principal: AdminPrincipal::LocalConsole,
                    permission: AdminPermission::UpgradeRuntime,
                })
                .expect("permission denied response should render"),
            "permission denied: principal=local-console permission=upgrade-runtime"
        );
    }
}
