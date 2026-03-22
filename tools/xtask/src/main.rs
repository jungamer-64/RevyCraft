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
    config_path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerConfigDocument {
    plugins: PluginConfigDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginConfigDocument {
    allowlist: Option<Vec<String>>,
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
        "  cargo run -p xtask -- package-plugins [--release] [--dist-dir <path>] [--config <path>]",
        "  cargo run -p xtask -- package-all-plugins [--release] [--dist-dir <path>]",
    ]
    .join("\n")
}

fn package_plugins(args: &[String]) -> Result<(), String> {
    let package_args = parse_package_args(args)?;
    let workspace_root = workspace_root()?;
    let discovered_plugins = discover_plugins(&workspace_root)?;
    let config_path =
        resolve_package_config_path(&workspace_root, package_args.config_path.as_deref())?;
    let plugin_allowlist = plugin_allowlist_from_toml(&config_path)?;
    println!(
        "using plugin allowlist from {}: {}",
        config_path.display(),
        plugin_allowlist.join(", ")
    );
    let plugins = filter_plugins_by_ids(discovered_plugins.clone(), &plugin_allowlist)?;
    package_plugin_specs(
        &workspace_root,
        &plugins,
        &managed_plugin_ids(&discovered_plugins),
        &package_args,
    )
}

fn package_all_plugins(args: &[String]) -> Result<(), String> {
    let package_args = parse_package_args(args)?;
    let workspace_root = workspace_root()?;
    let plugins = discover_plugins(&workspace_root)?;
    package_plugin_specs(
        &workspace_root,
        &plugins,
        &managed_plugin_ids(&plugins),
        &package_args,
    )
}

fn parse_package_args(args: &[String]) -> Result<PackageArgs, String> {
    let mut release = false;
    let mut dist_dir = PathBuf::from("runtime/plugins");
    let mut config_path = None;
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
            "--config" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--config requires a value".to_string());
                };
                config_path = Some(PathBuf::from(value));
                index += 2;
            }
            unknown => {
                return Err(format!("unknown xtask option `{unknown}`"));
            }
        }
    }
    Ok(PackageArgs {
        dist_dir,
        release,
        config_path,
    })
}

fn package_plugin_specs(
    workspace_root: &Path,
    plugins: &[PluginSpec],
    managed_plugin_ids: &BTreeSet<String>,
    package_args: &PackageArgs,
) -> Result<(), String> {
    let dist_dir = workspace_root.join(&package_args.dist_dir);
    fs::create_dir_all(&dist_dir).map_err(|error| error.to_string())?;
    let stage_dir = staging_dir_for_dist_dir(&dist_dir);
    remove_path_if_exists(&stage_dir)?;
    fs::create_dir_all(&stage_dir).map_err(|error| {
        format!(
            "failed to create staging dir {}: {error}",
            stage_dir.display()
        )
    })?;

    let selected_plugin_ids = plugins
        .iter()
        .map(|plugin| plugin.plugin_id.clone())
        .collect::<BTreeSet<_>>();
    let result = (|| -> Result<(), String> {
        for plugin in plugins {
            build_plugin(workspace_root, plugin, package_args.release)?;
            package_plugin(workspace_root, &stage_dir, plugin, package_args.release)?;
        }
        reconcile_packaged_plugins(
            &dist_dir,
            &stage_dir,
            managed_plugin_ids,
            &selected_plugin_ids,
        )
    })();
    let cleanup_result = remove_path_if_exists(&stage_dir);
    result?;
    cleanup_result?;

    println!("packaged plugins into {}", dist_dir.display());
    Ok(())
}

fn managed_plugin_ids(plugins: &[PluginSpec]) -> BTreeSet<String> {
    plugins
        .iter()
        .map(|plugin| plugin.plugin_id.clone())
        .collect()
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

fn resolve_package_config_path(
    workspace_root: &Path,
    explicit_path: Option<&Path>,
) -> Result<PathBuf, String> {
    let resolve = |path: &Path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace_root.join(path)
        }
    };
    if let Some(path) = explicit_path {
        let resolved = resolve(path);
        if resolved.is_file() {
            return Ok(resolved);
        }
        return Err(format!("config file {} does not exist", resolved.display()));
    }

    let active_config = workspace_root.join("runtime").join("server.toml");
    if active_config.is_file() {
        return Ok(active_config);
    }

    let example_config = workspace_root.join("runtime").join("server.toml.example");
    if example_config.is_file() {
        return Ok(example_config);
    }

    Err(format!(
        "no default config file found under {}",
        workspace_root.join("runtime").display()
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

fn plugin_allowlist_from_toml(config_path: &Path) -> Result<Vec<String>, String> {
    let contents = fs::read_to_string(config_path)
        .map_err(|error| format!("failed to read config {}: {error}", config_path.display()))?;
    let document: ServerConfigDocument = toml::from_str(&contents)
        .map_err(|error| format!("failed to parse config {}: {error}", config_path.display()))?;
    let raw_ids = document.plugins.allowlist.ok_or_else(|| {
        format!(
            "config {} did not define plugins.allowlist",
            config_path.display()
        )
    })?;
    let mut seen = BTreeSet::new();
    let plugin_ids = raw_ids
        .into_iter()
        .map(|plugin_id| plugin_id.trim().to_string())
        .filter(|plugin_id| !plugin_id.is_empty())
        .filter(|plugin_id| seen.insert(plugin_id.clone()))
        .collect::<Vec<_>>();
    if plugin_ids.is_empty() {
        return Err("plugins.allowlist was empty".to_string());
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
            "plugin allowlist referenced unknown plugin ids: {}",
            missing.join(", ")
        ));
    }
    Ok(filtered)
}

fn reconcile_packaged_plugins(
    dist_dir: &Path,
    stage_dir: &Path,
    managed_plugin_ids: &BTreeSet<String>,
    selected_plugin_ids: &BTreeSet<String>,
) -> Result<(), String> {
    for plugin_id in managed_plugin_ids {
        if selected_plugin_ids.contains(plugin_id) {
            continue;
        }
        remove_path_if_exists(&dist_dir.join(plugin_id))?;
    }

    for plugin_id in selected_plugin_ids {
        let staged_plugin_dir = stage_dir.join(plugin_id);
        if !staged_plugin_dir.is_dir() {
            return Err(format!(
                "staged plugin dir {} was not created",
                staged_plugin_dir.display()
            ));
        }
        let dist_plugin_dir = dist_dir.join(plugin_id);
        remove_path_if_exists(&dist_plugin_dir)?;
        fs::rename(&staged_plugin_dir, &dist_plugin_dir).map_err(|error| {
            format!(
                "failed to move {} into {}: {error}",
                staged_plugin_dir.display(),
                dist_plugin_dir.display()
            )
        })?;
    }

    Ok(())
}

fn staging_dir_for_dist_dir(dist_dir: &Path) -> PathBuf {
    let parent = dist_dir.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".revycraft-xtask-staging-{}", std::process::id()))
}

fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("failed to remove {}: {error}", path.display()))
    } else {
        fs::remove_file(path)
            .map_err(|error| format!("failed to remove {}: {error}", path.display()))
    }
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
    } else if rest.starts_with("admin-ui-") {
        ("admin-ui", rest.to_string())
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
        parse_package_args, plugin_allowlist_from_toml, plugin_spec_from_package_name,
        reconcile_packaged_plugins, resolve_package_config_path,
    };
    use std::collections::BTreeSet;
    use std::fs;
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
        assert_eq!(
            plugin_spec_from_package_name("mc-plugin-admin-ui-console")
                .expect("valid admin-ui plugin"),
            PluginSpec {
                cargo_package: "mc-plugin-admin-ui-console".to_string(),
                plugin_id: "admin-ui-console".to_string(),
                plugin_kind: "admin-ui".to_string(),
            }
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
    fn parse_package_args_reads_release_dist_dir_and_config() {
        assert_eq!(
            parse_package_args(&[
                "--release".to_string(),
                "--dist-dir".to_string(),
                "target/plugins".to_string(),
                "--config".to_string(),
                "runtime/server.toml".to_string(),
            ])
            .expect("xtask args should parse"),
            PackageArgs {
                dist_dir: PathBuf::from("target/plugins"),
                release: true,
                config_path: Some(PathBuf::from("runtime/server.toml")),
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

    #[test]
    fn resolve_package_config_path_prefers_active_runtime_toml() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let runtime_dir = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should be created");
        let active = runtime_dir.join("server.toml");
        let example = runtime_dir.join("server.toml.example");
        fs::write(&active, "[plugins]\nallowlist = [\"je-1_7_10\"]\n")
            .expect("active config should be written");
        fs::write(&example, "[plugins]\nallowlist = [\"je-1_8_x\"]\n")
            .expect("example config should be written");

        assert_eq!(
            resolve_package_config_path(temp_dir.path(), None)
                .expect("active config should be preferred"),
            active
        );
    }

    #[test]
    fn resolve_package_config_path_falls_back_to_example() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let runtime_dir = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should be created");
        let example = runtime_dir.join("server.toml.example");
        fs::write(&example, "[plugins]\nallowlist = [\"je-1_8_x\"]\n")
            .expect("example config should be written");

        assert_eq!(
            resolve_package_config_path(temp_dir.path(), None)
                .expect("example config should be used as fallback"),
            example
        );
    }

    #[test]
    fn plugin_allowlist_from_toml_reads_deduplicated_ids() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let config = temp_dir.path().join("server.toml");
        fs::write(
            &config,
            "[plugins]\nallowlist = [\"je-1_7_10\", \"je-1_7_10\", \"auth-offline\"]\n",
        )
        .expect("config should be written");

        assert_eq!(
            plugin_allowlist_from_toml(&config).expect("allowlist should be parsed successfully"),
            vec!["je-1_7_10".to_string(), "auth-offline".to_string()]
        );
    }

    #[test]
    fn reconcile_packaged_plugins_keeps_third_party_plugins_and_removes_unselected_managed() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        let stage_dir = temp_dir.path().join("runtime").join(".stage");
        fs::create_dir_all(&dist_dir).expect("dist dir should be created");
        fs::create_dir_all(&stage_dir).expect("stage dir should be created");

        let selected_dist_dir = dist_dir.join("je-1_7_10");
        fs::create_dir_all(&selected_dist_dir).expect("selected dist dir should be created");
        fs::write(selected_dist_dir.join("stale.txt"), "stale")
            .expect("stale file should be written");

        let removed_dist_dir = dist_dir.join("je-1_8_x");
        fs::create_dir_all(&removed_dist_dir).expect("removed dist dir should be created");
        fs::write(removed_dist_dir.join("plugin.toml"), "old")
            .expect("old manifest should be written");

        let third_party_dir = dist_dir.join("third-party");
        fs::create_dir_all(&third_party_dir).expect("third-party dir should be created");
        fs::write(third_party_dir.join("plugin.toml"), "external")
            .expect("external manifest should be written");

        let staged_plugin_dir = stage_dir.join("je-1_7_10");
        fs::create_dir_all(&staged_plugin_dir).expect("staged plugin dir should be created");
        fs::write(staged_plugin_dir.join("plugin.toml"), "new")
            .expect("new manifest should be written");

        reconcile_packaged_plugins(
            &dist_dir,
            &stage_dir,
            &["je-1_7_10".to_string(), "je-1_8_x".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            &["je-1_7_10".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
        .expect("reconcile should succeed");

        assert_eq!(
            fs::read_to_string(dist_dir.join("je-1_7_10").join("plugin.toml"))
                .expect("selected plugin manifest should exist"),
            "new"
        );
        assert!(
            !dist_dir.join("je-1_7_10").join("stale.txt").exists(),
            "selected managed plugin should be fully replaced"
        );
        assert!(
            !dist_dir.join("je-1_8_x").exists(),
            "unselected managed plugin should be removed"
        );
        assert!(
            dist_dir.join("third-party").exists(),
            "third-party plugin dirs should be preserved"
        );
    }

    fn tempdir() -> std::io::Result<tempfile::TempDir> {
        let base_dir = workspace_test_temp_root().join("xtask");
        fs::create_dir_all(&base_dir)?;
        tempfile::Builder::new()
            .prefix("xtask-")
            .tempdir_in(base_dir)
    }

    fn workspace_test_temp_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for ancestor in manifest_dir.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            if !manifest.is_file() {
                continue;
            }
            let Ok(contents) = fs::read_to_string(&manifest) else {
                continue;
            };
            if contents.contains("[workspace]") {
                return ancestor.join("target").join("test-tmp");
            }
        }
        panic!(
            "xtask tests should run under the workspace root: {}",
            manifest_dir.display()
        );
    }
}
