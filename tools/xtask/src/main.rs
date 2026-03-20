#![allow(clippy::multiple_crate_versions)]
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PluginSpec {
    cargo_package: String,
    plugin_id: String,
    plugin_kind: String,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    kind: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageArgs {
    dist_dir: PathBuf,
    release: bool,
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(help());
    };

    match command.as_str() {
        "package-plugins" => package_plugins(&args.collect::<Vec<_>>()),
        "package-all-plugins" => package_all_plugins(&args.collect::<Vec<_>>()),
        _ => Err(help()),
    }
}

fn help() -> String {
    [
        "usage:",
        "  cargo run -p xtask -- package-plugins [--release] [--dist-dir <path>]",
        "  cargo run -p xtask -- package-all-plugins [--release] [--dist-dir <path>]",
    ]
    .join("\n")
}

fn package_plugins(args: &[String]) -> Result<(), String> {
    let package_args = parse_package_args(args)?;
    let workspace_root = workspace_root()?;
    let plugins = filter_plugins_by_ids(
        discover_plugins(&workspace_root)?,
        &sample_plugin_allowlist(&workspace_root)?,
    )?;
    package_plugin_specs(&workspace_root, &plugins, &package_args)
}

fn package_all_plugins(args: &[String]) -> Result<(), String> {
    let package_args = parse_package_args(args)?;
    let workspace_root = workspace_root()?;
    let plugins = discover_plugins(&workspace_root)?;
    package_plugin_specs(&workspace_root, &plugins, &package_args)
}

fn parse_package_args(args: &[String]) -> Result<PackageArgs, String> {
    let mut release = false;
    let mut dist_dir = PathBuf::from("runtime/plugins");
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
    Ok(PackageArgs { dist_dir, release })
}

fn package_plugin_specs(
    workspace_root: &Path,
    plugins: &[PluginSpec],
    package_args: &PackageArgs,
) -> Result<(), String> {
    let dist_dir = workspace_root.join(&package_args.dist_dir);
    if dist_dir.exists() {
        fs::remove_dir_all(&dist_dir).map_err(|error| {
            format!(
                "failed to remove existing plugin dist dir {}: {error}",
                dist_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&dist_dir).map_err(|error| error.to_string())?;

    for plugin in plugins {
        build_plugin(workspace_root, plugin, package_args.release)?;
        package_plugin(workspace_root, &dist_dir, plugin, package_args.release)?;
    }

    println!("packaged plugins into {}", dist_dir.display());
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&manifest).map_err(|error| {
            format!(
                "failed to read workspace manifest candidate {}: {error}",
                manifest.display()
            )
        })?;
        if contents.contains("[workspace]") {
            return Ok(ancestor.to_path_buf());
        }
    }
    Err(format!(
        "failed to locate workspace root from {}",
        manifest_dir.display()
    ))
}

fn discover_plugins(workspace_root: &Path) -> Result<Vec<PluginSpec>, String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .current_dir(workspace_root)
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo metadata failed: {stderr}"));
    }

    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse cargo metadata: {error}"))?;
    let plugins_root = workspace_root.join("plugins");
    let mut plugins = metadata
        .packages
        .into_iter()
        .filter(|package| is_plugin_package(package, &plugins_root))
        .map(|package| plugin_spec_from_package_name(&package.name))
        .collect::<Result<Vec<_>, _>>()?;
    plugins.sort_by(|left, right| left.cargo_package.cmp(&right.cargo_package));
    Ok(plugins)
}

fn sample_plugin_allowlist(workspace_root: &Path) -> Result<Vec<String>, String> {
    let properties_path = workspace_root
        .join("runtime")
        .join("server.properties.example");
    let contents = fs::read_to_string(&properties_path).map_err(|error| {
        format!(
            "failed to read sample server properties {}: {error}",
            properties_path.display()
        )
    })?;
    let allowlist_line = contents
        .lines()
        .map(str::trim)
        .find(|line| !line.starts_with('#') && line.starts_with("plugin-allowlist="))
        .ok_or_else(|| {
            format!(
                "sample server properties {} did not define plugin-allowlist",
                properties_path.display()
            )
        })?;
    let (_, raw_ids) = allowlist_line
        .split_once('=')
        .ok_or_else(|| "plugin-allowlist line was malformed".to_string())?;
    let mut seen = BTreeSet::new();
    let plugin_ids = raw_ids
        .split(',')
        .map(str::trim)
        .filter(|plugin_id| !plugin_id.is_empty())
        .filter(|plugin_id| seen.insert((*plugin_id).to_string()))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if plugin_ids.is_empty() {
        return Err("plugin-allowlist was empty".to_string());
    }
    Ok(plugin_ids)
}

fn filter_plugins_by_ids(
    plugins: Vec<PluginSpec>,
    plugin_ids: &[String],
) -> Result<Vec<PluginSpec>, String> {
    let requested = plugin_ids.iter().cloned().collect::<BTreeSet<_>>();
    let filtered = plugins
        .into_iter()
        .filter(|plugin| requested.contains(&plugin.plugin_id))
        .collect::<Vec<_>>();
    let discovered = filtered
        .iter()
        .map(|plugin| plugin.plugin_id.clone())
        .collect::<BTreeSet<_>>();
    let missing = requested
        .difference(&discovered)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "sample allowlist referenced unknown plugin ids: {}",
            missing.join(", ")
        ));
    }
    Ok(filtered)
}

fn is_plugin_package(package: &CargoPackage, plugins_root: &Path) -> bool {
    package.name.starts_with("mc-plugin-")
        && !package.name.ends_with("-reload-test")
        && package
            .targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "cdylib"))
        && package
            .manifest_path
            .parent()
            .is_some_and(|parent| parent.starts_with(plugins_root))
}

fn plugin_spec_from_package_name(package_name: &str) -> Result<PluginSpec, String> {
    let Some(rest) = package_name.strip_prefix("mc-plugin-") else {
        return Err(format!("unsupported plugin package `{package_name}`"));
    };
    let (plugin_kind, plugin_id) = if let Some(adapter_id) = rest.strip_prefix("proto-") {
        ("protocol", adapter_id.to_string())
    } else if rest.starts_with("gameplay-") {
        ("gameplay", rest.to_string())
    } else if rest.starts_with("storage-") {
        ("storage", rest.to_string())
    } else if rest.starts_with("auth-") {
        ("auth", rest.to_string())
    } else {
        return Err(format!(
            "unsupported plugin package layout `{package_name}`"
        ));
    };
    Ok(PluginSpec {
        cargo_package: package_name.to_string(),
        plugin_id,
        plugin_kind: plugin_kind.to_string(),
    })
}

fn build_plugin(workspace_root: &Path, plugin: &PluginSpec, release: bool) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace_root)
        .arg("build")
        .arg("-p")
        .arg(&plugin.cargo_package);
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
        .join(dynamic_library_filename(&plugin.cargo_package));
    if !source.exists() {
        return Err(format!(
            "expected built plugin artifact `{}`",
            source.display()
        ));
    }

    let plugin_dir = dist_dir.join(&plugin.plugin_id);
    fs::create_dir_all(&plugin_dir).map_err(|error| error.to_string())?;

    let packaged_artifact_name = packaged_artifact_name(
        source
            .file_name()
            .ok_or_else(|| "plugin artifact had no file name".to_string())?
            .to_string_lossy()
            .as_ref(),
    );
    let destination = plugin_dir.join(&packaged_artifact_name);
    let staging = plugin_dir.join(format!(".{packaged_artifact_name}.tmp"));
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
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace_root.join("target"), PathBuf::from)
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
    packaged_artifact_name_with_tag(base_name, env::var("REVY_PLUGIN_BUILD_TAG").ok())
}

fn packaged_artifact_name_with_tag(base_name: &str, build_tag: Option<String>) -> String {
    match build_tag {
        Some(build_tag) if !build_tag.is_empty() => {
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

#[cfg(test)]
mod tests {
    use super::{
        PackageArgs, PluginSpec, filter_plugins_by_ids, packaged_artifact_name_with_tag,
        parse_package_args, plugin_spec_from_package_name,
    };
    use std::path::PathBuf;

    #[test]
    fn plugin_spec_maps_protocol_packages_to_adapter_ids() {
        assert_eq!(
            plugin_spec_from_package_name("mc-plugin-proto-je-1_12_2")
                .expect("valid protocol plugin"),
            PluginSpec {
                cargo_package: "mc-plugin-proto-je-1_12_2".to_string(),
                plugin_id: "je-1_12_2".to_string(),
                plugin_kind: "protocol".to_string(),
            }
        );
    }

    #[test]
    fn plugin_spec_keeps_non_protocol_prefixes_in_plugin_id() {
        assert_eq!(
            plugin_spec_from_package_name("mc-plugin-auth-bedrock-offline")
                .expect("valid auth plugin")
                .plugin_id,
            "auth-bedrock-offline"
        );
        assert_eq!(
            plugin_spec_from_package_name("mc-plugin-gameplay-canonical")
                .expect("valid gameplay plugin")
                .plugin_id,
            "gameplay-canonical"
        );
    }

    #[test]
    fn packaged_artifact_name_appends_sanitized_build_tag() {
        assert_eq!(
            packaged_artifact_name_with_tag("libmc_plugin.so", Some("reload smoke".to_string())),
            "libmc_plugin-reload_smoke.so"
        );
    }

    #[test]
    fn parse_package_args_reads_release_and_dist_dir() {
        assert_eq!(
            parse_package_args(&[
                "--release".to_string(),
                "--dist-dir".to_string(),
                "target/plugins".to_string(),
            ])
            .expect("xtask args should parse"),
            PackageArgs {
                dist_dir: PathBuf::from("target/plugins"),
                release: true,
            }
        );
    }

    #[test]
    fn filter_plugins_by_ids_rejects_unknown_sample_plugin() {
        let error = filter_plugins_by_ids(
            vec![PluginSpec {
                cargo_package: "mc-plugin-proto-je-1_7_10".to_string(),
                plugin_id: "je-1_7_10".to_string(),
                plugin_kind: "protocol".to_string(),
            }],
            &["missing-plugin".to_string()],
        )
        .expect_err("unknown sample plugin ids should fail");
        assert!(error.contains("missing-plugin"));
    }
}
