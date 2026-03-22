mod build;
mod network;
mod packets;
mod plugins;

use super::*;
use crate::runtime::RunningServer;

pub(crate) use self::build::*;
pub(crate) use self::network::*;
pub(crate) use self::packets::*;
pub(crate) use self::plugins::*;

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

pub(crate) async fn loaded_plugins_snapshot(server: &RunningServer) -> LoadedPluginSet {
    server
        .runtime
        .live_state
        .read()
        .await
        .loaded_plugins
        .clone()
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn toml_array(values: &[String]) -> String {
    toml::Value::Array(
        values
            .iter()
            .cloned()
            .map(toml::Value::String)
            .collect::<Vec<_>>(),
    )
    .to_string()
}

fn failure_policy_name(action: PluginFailureAction) -> &'static str {
    match action {
        PluginFailureAction::Skip => "skip",
        PluginFailureAction::Quarantine => "quarantine",
        PluginFailureAction::FailFast => "fail-fast",
    }
}

fn admin_permissions_toml(permissions: &[crate::runtime::AdminPermission]) -> String {
    toml::Value::Array(
        permissions
            .iter()
            .map(|permission| toml::Value::String(permission.as_str().to_string()))
            .collect::<Vec<_>>(),
    )
    .to_string()
}

pub(crate) fn write_server_toml(path: &Path, config: &ServerConfig) -> Result<(), RuntimeError> {
    let mut contents = String::new();
    contents.push_str("[bootstrap]\n");
    contents.push_str(&format!(
        "online_mode = {}\nlevel_name = {}\nlevel_type = {}\ngame_mode = {}\ndifficulty = {}\nview_distance = {}\nworld_dir = {}\nstorage_profile = {}\nplugins_dir = {}\nplugin_abi_min = {}\nplugin_abi_max = {}\n\n",
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
        toml_string(&config.bootstrap.plugins_dir.display().to_string()),
        toml_string(&config.bootstrap.plugin_abi_min.to_string()),
        toml_string(&config.bootstrap.plugin_abi_max.to_string()),
    ));
    contents.push_str("[network]\n");
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
    contents.push_str("[topology]\n");
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
    contents.push_str("[plugins]\n");
    contents.push_str(&format!("reload_watch = {}\n", config.plugins.reload_watch));
    if let Some(allowlist) = &config.plugins.allowlist {
        contents.push_str(&format!("allowlist = {}\n", toml_array(allowlist)));
    }
    contents.push_str("\n[plugins.failure_policy]\n");
    contents.push_str(&format!(
        "protocol = {}\ngameplay = {}\nstorage = {}\nauth = {}\nadmin_ui = {}\n\n",
        toml_string(failure_policy_name(config.plugins.failure_policy.protocol)),
        toml_string(failure_policy_name(config.plugins.failure_policy.gameplay)),
        toml_string(failure_policy_name(config.plugins.failure_policy.storage)),
        toml_string(failure_policy_name(config.plugins.failure_policy.auth)),
        toml_string(failure_policy_name(config.plugins.failure_policy.admin_ui)),
    ));
    contents.push_str("[profiles]\n");
    contents.push_str(&format!(
        "auth = {}\nbedrock_auth = {}\ndefault_gameplay = {}\n\n",
        toml_string(&config.profiles.auth),
        toml_string(&config.profiles.bedrock_auth),
        toml_string(&config.profiles.default_gameplay),
    ));
    contents.push_str("[profiles.gameplay_map]\n");
    let mut gameplay_entries = config.profiles.gameplay_map.iter().collect::<Vec<_>>();
    gameplay_entries.sort_by(|left, right| left.0.cmp(right.0));
    for (adapter_id, profile_id) in gameplay_entries {
        contents.push_str(&format!(
            "{} = {}\n",
            toml_string(adapter_id),
            toml_string(profile_id)
        ));
    }
    contents.push_str("\n[admin]\n");
    contents.push_str(&format!(
        "ui_profile = {}\nlocal_console_permissions = {}\n",
        toml_string(&config.admin.ui_profile),
        admin_permissions_toml(&config.admin.local_console_permissions),
    ));
    contents.push_str("\n[admin.grpc]\n");
    contents.push_str(&format!(
        "enabled = {}\nbind_addr = {}\nallow_non_loopback = {}\n",
        config.admin.grpc.enabled,
        toml_string(&config.admin.grpc.bind_addr.to_string()),
        config.admin.grpc.allow_non_loopback,
    ));
    let mut principals = config.admin.grpc.principals.iter().collect::<Vec<_>>();
    principals.sort_by(|left, right| left.0.cmp(right.0));
    for (principal_id, principal) in principals {
        contents.push_str(&format!(
            "\n[admin.grpc.principals.{}]\n",
            toml_string(principal_id)
        ));
        contents.push_str(&format!(
            "token_file = {}\npermissions = {}\n",
            toml_string(&principal.token_file.display().to_string()),
            admin_permissions_toml(&principal.permissions),
        ));
    }
    fs::write(path, contents)?;
    Ok(())
}
