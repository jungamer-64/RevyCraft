#![allow(clippy::multiple_crate_versions)]
use mc_plugin_host::host::plugin_host_from_config;
use server_runtime::RuntimeError;
use server_runtime::config::{ServerConfig, ServerConfigSource};
use server_runtime::runtime::{
    AdminPrincipal, AdminResponse, ServerBuilder, format_runtime_status_summary,
};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

async fn wait_for_ctrl_c() -> Result<(), RuntimeError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))
}

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_toml(Path::new("runtime/server.toml"))?;
    let plugin_host =
        plugin_host_from_config(&config.plugin_host_bootstrap_config())?.ok_or_else(|| {
            RuntimeError::Config(format!(
                "no packaged plugins discovered under `{}`",
                config.bootstrap.plugins_dir.display()
            ))
        })?;
    let loaded_plugins =
        plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;

    let server = ServerBuilder::new(
        ServerConfigSource::Toml(Path::new("runtime/server.toml").to_path_buf()),
        loaded_plugins,
    )
    .with_reload_host(plugin_host)
    .build()
    .await?;
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    println!("{}", format_runtime_status_summary(&server.status().await));
    if server.admin_ui().await.is_some() {
        let control_plane = server.admin_control_plane();
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        loop {
            tokio::select! {
                signal = wait_for_ctrl_c() => {
                    signal?;
                    break;
                }
                line = lines.next_line() => {
                    let Some(line) = line.map_err(|error| RuntimeError::Config(format!("failed to read stdin: {error}")))? else {
                        break;
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Some(ui) = server.admin_ui().await else {
                        eprintln!("admin-ui became unavailable; console commands are disabled");
                        continue;
                    };
                    let request = match ui.parse_line(line) {
                        Ok(request) => request,
                        Err(error) => {
                            let response = AdminResponse::Error { message: error.to_string() };
                            match ui.render_response(&response) {
                                Ok(text) => println!("{text}"),
                                Err(render_error) => eprintln!("{render_error}"),
                            }
                            continue;
                        }
                    };
                    let response = control_plane.execute(AdminPrincipal::LocalConsole, request).await;
                    let shutdown_requested = matches!(response, AdminResponse::ShutdownScheduled);
                    match ui.render_response(&response) {
                        Ok(text) => println!("{text}"),
                        Err(error) => eprintln!("{error}"),
                    }
                    if shutdown_requested {
                        break;
                    }
                }
            }
        }
    } else {
        eprintln!("admin-ui unavailable at boot; stdio control loop is disabled");
        wait_for_ctrl_c().await?;
    }
    server.shutdown().await
}
