mod build;
mod network;
mod packets;
mod plugins;

use super::*;
use crate::runtime::RunningServer;
use std::time::{Duration, Instant, SystemTime};

pub(crate) use self::build::*;
pub(crate) use self::network::*;
pub(crate) use self::packets::*;
pub(crate) use self::plugins::*;

pub(crate) const CONSOLE_SURFACE_ID: &str = "console";
pub(crate) const REMOTE_SURFACE_ID: &str = "remote";
pub(crate) const CONSOLE_PRINCIPAL_ID: &str = "console:console";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UdpDatagramAction {
    Ignore,
    UnsupportedBedrock,
}

pub(crate) fn classify_udp_datagram(
    protocol_registry: &ProtocolRegistry,
    datagram: &[u8],
) -> Result<UdpDatagramAction, ProtocolError> {
    match protocol_registry.route_handshake(TransportKind::Udp, datagram)? {
        Some(intent) if intent.edition == Edition::Be => Ok(UdpDatagramAction::UnsupportedBedrock),
        Some(_) | None => Ok(UdpDatagramAction::Ignore),
    }
}

pub(crate) fn loopback_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = ServerConfig::default();
    config.network.server_ip = Some("127.0.0.1".parse().expect("loopback should parse"));
    config.network.server_port = 0;
    config.bootstrap.world_dir = world_dir;
    config
}

pub(crate) fn set_console_surface(config: &mut ServerConfig, profile: &str) {
    config.admin.surfaces.insert(
        CONSOLE_SURFACE_ID.to_string(),
        crate::config::AdminSurfaceConfig {
            profile: profile.into(),
            config: None,
        },
    );
}

pub(crate) fn set_remote_surface(
    config: &mut ServerConfig,
    profile: &str,
    surface_config_path: impl Into<PathBuf>,
) {
    config.admin.surfaces.insert(
        REMOTE_SURFACE_ID.to_string(),
        crate::config::AdminSurfaceConfig {
            profile: profile.into(),
            config: Some(surface_config_path.into()),
        },
    );
}

pub(crate) fn set_console_permissions(
    config: &mut ServerConfig,
    permissions: Vec<crate::config::AdminPermission>,
) {
    config.admin.principals.insert(
        CONSOLE_PRINCIPAL_ID.to_string(),
        crate::config::AdminPrincipalConfig { permissions },
    );
}

pub(crate) fn console_permissions(
    config: &ServerConfig,
) -> Option<&Vec<crate::config::AdminPermission>> {
    config
        .admin
        .principals
        .get(CONSOLE_PRINCIPAL_ID)
        .map(|principal| &principal.permissions)
}

pub(crate) fn console_surface(config: &ServerConfig) -> Option<&crate::config::AdminSurfaceConfig> {
    config.admin.surfaces.get(CONSOLE_SURFACE_ID)
}

pub(crate) fn remote_surface(config: &ServerConfig) -> Option<&crate::config::AdminSurfaceConfig> {
    config.admin.surfaces.get(REMOTE_SURFACE_ID)
}

pub(crate) fn seed_runtime_plugins_with_loopback_admin(
    config: &mut ServerConfig,
    dist_dir: &Path,
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
    token_root: &Path,
    principal_id: &str,
    token: &str,
    permissions: Vec<crate::config::AdminPermission>,
    bind_addr: std::net::SocketAddr,
) -> Result<PathBuf, RuntimeError> {
    seed_runtime_plugins(dist_dir, allowlist, supporting_plugin_ids)?;
    config.bootstrap.plugins_dir = dist_dir.to_path_buf();
    let token_path = token_root.join(format!("{principal_id}.token"));
    if let Some(parent) = token_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&token_path, format!("{token}\n"))?;
    let grpc_surface_config_path = token_root.join("admin-grpc.toml");
    let grpc_surface_config_contents = format!(
        "bind_addr = {}\nallow_non_loopback = false\n\n[principals.{}]\ntoken_file = {}\n",
        toml_string(&bind_addr.to_string()),
        toml_string(principal_id),
        toml_string(
            token_path
                .file_name()
                .expect("token file should have a file name")
                .to_string_lossy()
        ),
    );
    fs::write(&grpc_surface_config_path, grpc_surface_config_contents)?;
    set_remote_surface(config, "grpc-v1", grpc_surface_config_path);
    config.admin.principals.insert(
        principal_id.to_string(),
        crate::config::AdminPrincipalConfig { permissions },
    );
    Ok(token_path)
}

pub(crate) async fn loaded_plugins_snapshot(server: &RunningServer) -> LoadedPluginSet {
    server.runtime.selection_state().await.loaded_plugins
}

fn toml_string(value: impl AsRef<str>) -> String {
    format!("{:?}", value.as_ref())
}

fn toml_array<T: AsRef<str>>(values: &[T]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(toml_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn failure_policy_name(action: PluginFailureAction) -> &'static str {
    match action {
        PluginFailureAction::Skip => "skip",
        PluginFailureAction::Quarantine => "quarantine",
        PluginFailureAction::FailFast => "fail-fast",
    }
}

fn admin_permissions_toml(permissions: &[crate::config::AdminPermission]) -> String {
    format!(
        "[{}]",
        permissions
            .iter()
            .map(|permission| toml_string(permission.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(crate) fn plugin_host_bootstrap_test_config(
    config: &ServerConfig,
) -> mc_plugin_host::config::BootstrapConfig {
    config.plugin_host_bootstrap_config()
}

pub(crate) fn plugin_host_runtime_selection_test_config(
    config: &ServerConfig,
) -> mc_plugin_host::config::RuntimeSelectionConfig {
    config.plugin_host_runtime_selection_config()
}

pub(crate) fn write_server_toml(path: &Path, config: &ServerConfig) -> Result<(), RuntimeError> {
    let mut contents = String::new();
    contents.push_str("[static.bootstrap]\n");
    contents.push_str(&format!(
        "online_mode = {}\nlevel_name = {}\nlevel_type = {}\ngame_mode = {}\ndifficulty = {}\nview_distance = {}\nworld_dir = {}\nstorage_profile = {}\n\n",
        config.bootstrap.online_mode,
        toml_string(&config.bootstrap.level_name),
        toml_string(match config.bootstrap.level_type {
            LevelType::Flat => "flat",
        }),
        config.bootstrap.game_mode,
        config.bootstrap.difficulty,
        config.bootstrap.view_distance,
        toml_string(&config.bootstrap.world_dir.display().to_string()),
        toml_string(&config.bootstrap.storage_profile),
    ));
    contents.push_str("[static.plugins]\n");
    contents.push_str(&format!(
        "plugins_dir = {}\nplugin_abi_min = {}\nplugin_abi_max = {}\n\n",
        toml_string(&config.bootstrap.plugins_dir.display().to_string()),
        toml_string(&config.bootstrap.plugin_abi_min.to_string()),
        toml_string(&config.bootstrap.plugin_abi_max.to_string()),
    ));
    let mut principals = config.admin.principals.iter().collect::<Vec<_>>();
    principals.sort_by(|left, right| left.0.cmp(right.0));
    for (principal_id, principal) in principals {
        contents.push_str(&format!(
            "[static.admin.principals.{}]\n",
            toml_string(principal_id)
        ));
        contents.push_str(&format!(
            "permissions = {}\n\n",
            admin_permissions_toml(&principal.permissions),
        ));
    }
    contents.push_str("\n[live.network]\n");
    if let Some(server_ip) = config.network.server_ip {
        contents.push_str(&format!(
            "server_ip = {}\n",
            toml_string(&server_ip.to_string())
        ));
    }
    contents.push_str(&format!(
        "server_port = {}\nmotd = {}\nmax_players = {}\n\n",
        config.network.server_port,
        toml_string(&config.network.motd),
        config.network.max_players,
    ));
    contents.push_str("[live.topology]\n");
    contents.push_str(&format!(
        "be_enabled = {}\ndefault_adapter = {}\ndefault_bedrock_adapter = {}\nreload_watch = {}\ndrain_grace_secs = {}\n",
        config.topology.be_enabled,
        toml_string(&config.topology.default_adapter),
        toml_string(&config.topology.default_bedrock_adapter),
        config.topology.reload_watch,
        config.topology.drain_grace_secs,
    ));
    if let Some(enabled_adapters) = &config.topology.enabled_adapters {
        contents.push_str(&format!(
            "enabled_adapters = {}\n",
            toml_array(enabled_adapters)
        ));
    }
    if let Some(enabled_bedrock_adapters) = &config.topology.enabled_bedrock_adapters {
        contents.push_str(&format!(
            "enabled_bedrock_adapters = {}\n",
            toml_array(enabled_bedrock_adapters)
        ));
    }
    contents.push('\n');
    contents.push_str("[live.plugins]\n");
    contents.push_str(&format!("reload_watch = {}\n", config.plugins.reload_watch));
    if let Some(allowlist) = &config.plugins.allowlist {
        contents.push_str(&format!("allowlist = {}\n", toml_array(allowlist)));
    }
    contents.push_str("\n[live.plugins.buffer_limits]\n");
    contents.push_str(&format!(
        "protocol_response_bytes = {}\ngameplay_response_bytes = {}\nstorage_response_bytes = {}\nauth_response_bytes = {}\nadmin_surface_response_bytes = {}\ncallback_payload_bytes = {}\nmetadata_bytes = {}\n",
        config.plugins.buffer_limits.protocol_response_bytes,
        config.plugins.buffer_limits.gameplay_response_bytes,
        config.plugins.buffer_limits.storage_response_bytes,
        config.plugins.buffer_limits.auth_response_bytes,
        config.plugins.buffer_limits.admin_surface_response_bytes,
        config.plugins.buffer_limits.callback_payload_bytes,
        config.plugins.buffer_limits.metadata_bytes,
    ));
    contents.push_str("\n[live.plugins.failure_policy]\n");
    contents.push_str(&format!(
        "protocol = {}\ngameplay = {}\nstorage = {}\nauth = {}\nadmin_surface = {}\n\n",
        toml_string(failure_policy_name(config.plugins.failure_policy.protocol)),
        toml_string(failure_policy_name(config.plugins.failure_policy.gameplay)),
        toml_string(failure_policy_name(config.plugins.failure_policy.storage)),
        toml_string(failure_policy_name(config.plugins.failure_policy.auth)),
        toml_string(failure_policy_name(
            config.plugins.failure_policy.admin_surface
        )),
    ));
    contents.push_str("[live.profiles]\n");
    contents.push_str(&format!(
        "auth = {}\nbedrock_auth = {}\ndefault_gameplay = {}\n\n",
        toml_string(&config.profiles.auth),
        toml_string(&config.profiles.bedrock_auth),
        toml_string(&config.profiles.default_gameplay),
    ));
    contents.push_str("[live.profiles.gameplay_map]\n");
    let mut gameplay_entries = config.profiles.gameplay_map.iter().collect::<Vec<_>>();
    gameplay_entries.sort_by(|left, right| left.0.cmp(right.0));
    for (adapter_id, profile_id) in gameplay_entries {
        contents.push_str(&format!(
            "{} = {}\n",
            toml_string(adapter_id),
            toml_string(profile_id)
        ));
    }
    let mut admin_surfaces = config.admin.surfaces.iter().collect::<Vec<_>>();
    admin_surfaces.sort_by(|left, right| left.0.cmp(right.0));
    for (instance_id, surface) in admin_surfaces {
        contents.push_str(&format!(
            "\n[live.admin.surfaces.{}]\nprofile = {}\n",
            toml_string(instance_id),
            toml_string(surface.profile.as_str()),
        ));
        if let Some(config_path) = &surface.config {
            contents.push_str(&format!(
                "config = {}\n",
                toml_string(&config_path.display().to_string())
            ));
        }
    }
    fs::write(path, contents)?;
    Ok(())
}

pub(crate) fn write_server_toml_for_reload(
    path: &Path,
    config: &ServerConfig,
) -> Result<(), RuntimeError> {
    let previous_modified_at = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    write_server_toml(path, config)?;
    ensure_file_reload_visible(path, previous_modified_at)
}

fn ensure_file_reload_visible(
    path: &Path,
    previous_modified_at: Option<SystemTime>,
) -> Result<(), RuntimeError> {
    let Some(previous_modified_at) = previous_modified_at else {
        return Ok(());
    };
    let contents = fs::read(path)?;
    let deadline = Instant::now() + Duration::from_secs(3);

    loop {
        let modified_at = fs::metadata(path)?.modified()?;
        if modified_at > previous_modified_at {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(RuntimeError::Config(format!(
                "timed out waiting for reload-visible config update at {}",
                path.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
        fs::write(path, &contents)?;
    }
}
