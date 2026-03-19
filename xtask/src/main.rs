use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct PluginSpec {
    cargo_package: &'static str,
    plugin_id: &'static str,
    plugin_kind: &'static str,
}

const PROTOCOL_PLUGINS: &[PluginSpec] = &[
    PluginSpec {
        cargo_package: "mc-plugin-proto-je-1_7_10",
        plugin_id: "je-1_7_10",
        plugin_kind: "protocol",
    },
    PluginSpec {
        cargo_package: "mc-plugin-proto-je-1_8_x",
        plugin_id: "je-1_8_x",
        plugin_kind: "protocol",
    },
    PluginSpec {
        cargo_package: "mc-plugin-proto-je-1_12_2",
        plugin_id: "je-1_12_2",
        plugin_kind: "protocol",
    },
    PluginSpec {
        cargo_package: "mc-plugin-proto-be-placeholder",
        plugin_id: "be-placeholder",
        plugin_kind: "protocol",
    },
];

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(help());
    };

    match command.as_str() {
        "package-plugins" => package_plugins(args.collect()),
        _ => Err(help()),
    }
}

fn help() -> String {
    "usage: cargo run -p xtask -- package-plugins [--release] [--dist-dir <path>]".to_string()
}

fn package_plugins(args: Vec<String>) -> Result<(), String> {
    let mut release = false;
    let mut dist_dir = PathBuf::from("dist/plugins");
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--release" => {
                release = true;
                index += 1;
            }
            "--dist-dir" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--dist-dir requires a value".to_string());
                };
                dist_dir = PathBuf::from(value);
                index += 2;
            }
            unknown => {
                return Err(format!("unknown xtask option `{unknown}`"));
            }
        }
    }

    let workspace_root = workspace_root()?;
    let dist_dir = workspace_root.join(dist_dir);
    fs::create_dir_all(&dist_dir).map_err(|error| error.to_string())?;

    for plugin in PROTOCOL_PLUGINS {
        build_plugin(&workspace_root, plugin, release)?;
        package_plugin(&workspace_root, &dist_dir, plugin, release)?;
    }

    println!("packaged protocol plugins into {}", dist_dir.display());
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask manifest did not have a workspace parent".to_string())
}

fn build_plugin(workspace_root: &Path, plugin: &PluginSpec, release: bool) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace_root)
        .arg("build")
        .arg("-p")
        .arg(plugin.cargo_package);
    if release {
        command.arg("--release");
    }
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        command.arg("--target-dir").arg(target_dir);
    }
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo build failed while building `{}`",
            plugin.cargo_package
        ))
    }
}

fn package_plugin(
    workspace_root: &Path,
    dist_dir: &Path,
    plugin: &PluginSpec,
    release: bool,
) -> Result<(), String> {
    let build_profile = if release { "release" } else { "debug" };
    let source = target_dir(workspace_root)
        .join(build_profile)
        .join(dynamic_library_filename(plugin.cargo_package));
    if !source.exists() {
        return Err(format!(
            "expected built plugin artifact `{}`",
            source.display()
        ));
    }

    let plugin_dir = dist_dir.join(plugin.plugin_id);
    fs::create_dir_all(&plugin_dir).map_err(|error| error.to_string())?;

    let packaged_artifact_name = packaged_artifact_name(
        source
            .file_name()
            .ok_or_else(|| "plugin artifact had no file name".to_string())?
            .to_string_lossy()
            .as_ref(),
    );
    let destination = plugin_dir.join(&packaged_artifact_name);
    let staging = plugin_dir.join(format!(
        ".{}.tmp",
        packaged_artifact_name
    ));
    fs::copy(&source, &staging).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            staging.display()
        )
    })?;
    if destination.exists() {
        fs::remove_file(&destination).map_err(|error| {
            format!(
                "failed to remove stale packaged artifact {}: {error}",
                destination.display()
            )
        })?;
    }
    fs::rename(&staging, &destination).map_err(|error| {
        format!(
            "failed to move {} into {}: {error}",
            staging.display(),
            destination.display()
        )
    })?;

    let manifest = format!(
        "[plugin]\nid = \"{}\"\nkind = \"{}\"\n\n[artifacts]\n\"{}-{}\" = \"{}\"\n",
        plugin.plugin_id,
        plugin.plugin_kind,
        env::consts::OS,
        env::consts::ARCH,
        packaged_artifact_name
    );
    fs::write(plugin_dir.join("plugin.toml"), manifest).map_err(|error| error.to_string())
}

fn target_dir(workspace_root: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"))
}

fn dynamic_library_filename(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    match env::consts::OS {
        "windows" => format!("{crate_name}.dll"),
        "macos" => format!("lib{crate_name}.dylib"),
        _ => format!("lib{crate_name}.so"),
    }
}

fn packaged_artifact_name(base_name: &str) -> String {
    match env::var("REVY_PLUGIN_BUILD_TAG") {
        Ok(build_tag) if !build_tag.is_empty() => {
            let sanitized = build_tag
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            if let Some((stem, extension)) = base_name.rsplit_once('.') {
                format!("{stem}-{sanitized}.{extension}")
            } else {
                format!("{base_name}-{sanitized}")
            }
        }
        _ => base_name.to_string(),
    }
}
