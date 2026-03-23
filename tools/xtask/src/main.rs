#![allow(clippy::multiple_crate_versions)]
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVER_BINARY_PACKAGE: &str = "server-bootstrap";

#[derive(Clone, Debug, PartialEq, Eq)]
struct PluginSpec {
    cargo_package: String,
    plugin_id: String,
    plugin_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BuildTarget {
    triple: String,
    os: String,
    arch: String,
    artifact_key: String,
    dylib_ext: String,
    exe_ext: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseBundleArgs {
    output_dir: PathBuf,
    config_path: PathBuf,
    include_example_config: bool,
    targets: Vec<BuildTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ServerConfigDocument {
    plugins: PluginConfigDocument,
    live: LiveConfigScopeDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LiveConfigScopeDocument {
    plugins: PluginConfigDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PluginConfigDocument {
    allowlist: Option<Vec<String>>,
}

impl BuildTarget {
    fn from_triple(triple: &str) -> Result<Self, String> {
        let arch = triple
            .split('-')
            .next()
            .ok_or_else(|| format!("target triple `{triple}` is missing an architecture"))?;
        let arch = match arch {
            "x86_64" | "aarch64" => arch,
            _ => {
                return Err(format!(
                    "unsupported target architecture `{arch}` in `{triple}`; supported architectures are x86_64 and aarch64"
                ));
            }
        };
        let os = if triple.contains("windows") {
            "windows"
        } else if triple.contains("linux") {
            "linux"
        } else if triple.contains("darwin") {
            "macos"
        } else {
            return Err(format!(
                "unsupported target operating system in `{triple}`; supported families are linux, windows, and darwin"
            ));
        };
        let dylib_ext = match os {
            "windows" => "dll",
            "macos" => "dylib",
            "linux" => "so",
            _ => unreachable!(),
        };
        let exe_ext = if os == "windows" { ".exe" } else { "" };
        Ok(Self {
            triple: triple.to_string(),
            os: os.to_string(),
            arch: arch.to_string(),
            artifact_key: artifact_key(os, arch),
            dylib_ext: dylib_ext.to_string(),
            exe_ext: exe_ext.to_string(),
        })
    }

    fn dynamic_library_filename(&self, package: &str) -> String {
        dynamic_library_filename_for_os(self.os.as_str(), package)
    }

    fn binary_filename(&self, name: &str) -> String {
        format!("{name}{}", self.exe_ext)
    }
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(help());
    };

    match command.as_str() {
        "package-plugins" => package_plugins(&args.collect::<Vec<_>>()),
        "package-all-plugins" => package_all_plugins(&args.collect::<Vec<_>>()),
        "build-release-bundles" => build_release_bundles(&args.collect::<Vec<_>>()),
        _ => Err(help()),
    }
}

fn help() -> String {
    [
        "usage:",
        "  cargo run -p xtask -- package-plugins [--release] [--dist-dir <path>] [--config <path>]",
        "  cargo run -p xtask -- package-all-plugins [--release] [--dist-dir <path>]",
        "  cargo run -p xtask -- build-release-bundles --target <triple>... [--output-dir <path>] [--config <path>]",
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

fn build_release_bundles(args: &[String]) -> Result<(), String> {
    let workspace_root = workspace_root()?;
    let release_args = parse_release_bundle_args(&workspace_root, args)?;
    let discovered_plugins = discover_plugins(&workspace_root)?;
    let plugin_allowlist = plugin_allowlist_from_toml(&release_args.config_path)?;
    println!(
        "using release bundle allowlist from {}: {}",
        release_args.config_path.display(),
        plugin_allowlist.join(", ")
    );
    let plugins = filter_plugins_by_ids(discovered_plugins, &plugin_allowlist)?;
    let output_root = workspace_root.join(&release_args.output_dir);

    run_release_bundle_jobs(&output_root, &release_args.targets, |target, bundle_dir| {
        build_release_bundle(
            &workspace_root,
            bundle_dir,
            target,
            &plugins,
            &release_args.config_path,
            release_args.include_example_config,
        )
    })
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

fn parse_release_bundle_args(
    workspace_root: &Path,
    args: &[String],
) -> Result<ReleaseBundleArgs, String> {
    let mut output_dir = PathBuf::from("dist").join("releases");
    let mut config_path = None;
    let mut targets = Vec::new();
    let mut seen_targets = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--target" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--target requires a value".to_string());
                };
                let target = BuildTarget::from_triple(value)?;
                if seen_targets.insert(target.triple.clone()) {
                    targets.push(target);
                }
                index += 2;
            }
            "--output-dir" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--output-dir requires a value".to_string());
                };
                output_dir = PathBuf::from(value);
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
    if targets.is_empty() {
        return Err("build-release-bundles requires at least one --target".to_string());
    }

    let resolved_config =
        resolve_release_bundle_config_path(workspace_root, config_path.as_deref())?;
    let default_example = workspace_root.join("runtime").join("server.toml.example");
    Ok(ReleaseBundleArgs {
        output_dir,
        include_example_config: resolved_config == default_example,
        config_path: resolved_config,
        targets,
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

fn run_release_bundle_jobs<F>(
    output_root: &Path,
    targets: &[BuildTarget],
    mut build_job: F,
) -> Result<(), String>
where
    F: FnMut(&BuildTarget, &Path) -> Result<(), String>,
{
    fs::create_dir_all(output_root).map_err(|error| {
        format!(
            "failed to create release output root {}: {error}",
            output_root.display()
        )
    })?;

    let mut failures = Vec::new();
    for target in targets {
        let bundle_dir = output_root.join(&target.triple);
        println!("building release bundle for {}", target.triple);
        match build_job(target, &bundle_dir) {
            Ok(()) => println!("built release bundle into {}", bundle_dir.display()),
            Err(error) => {
                eprintln!(
                    "failed to build release bundle for {}: {}",
                    target.triple, error
                );
                failures.push((target.triple.clone(), error));
            }
        }
    }

    println!(
        "release bundle summary: {} succeeded, {} failed",
        targets.len().saturating_sub(failures.len()),
        failures.len()
    );
    if failures.is_empty() {
        return Ok(());
    }

    Err(format!(
        "release bundle generation failed for: {}",
        failures
            .into_iter()
            .map(|(target, error)| format!("{target} ({error})"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn build_release_bundle(
    workspace_root: &Path,
    bundle_dir: &Path,
    target: &BuildTarget,
    plugins: &[PluginSpec],
    config_path: &Path,
    include_example_config: bool,
) -> Result<(), String> {
    build_server_binary(workspace_root, target)?;
    for plugin in plugins {
        build_plugin_for_target(workspace_root, plugin, target)?;
    }

    let stage_dir = staging_dir_for_dist_dir(bundle_dir);
    remove_path_if_exists(&stage_dir)?;
    fs::create_dir_all(&stage_dir).map_err(|error| {
        format!(
            "failed to create release bundle staging dir {}: {error}",
            stage_dir.display()
        )
    })?;

    let result = (|| -> Result<(), String> {
        stage_release_bundle(
            workspace_root,
            &stage_dir,
            target,
            plugins,
            config_path,
            include_example_config,
        )?;
        remove_path_if_exists(bundle_dir)?;
        fs::rename(&stage_dir, bundle_dir).map_err(|error| {
            format!(
                "failed to move staged release bundle {} into {}: {error}",
                stage_dir.display(),
                bundle_dir.display()
            )
        })
    })();
    if result.is_err() {
        let _ = remove_path_if_exists(&stage_dir);
    }
    result
}

fn stage_release_bundle(
    workspace_root: &Path,
    stage_dir: &Path,
    target: &BuildTarget,
    plugins: &[PluginSpec],
    config_path: &Path,
    include_example_config: bool,
) -> Result<(), String> {
    let runtime_dir = stage_dir.join("runtime");
    let plugins_dir = runtime_dir.join("plugins");
    fs::create_dir_all(&plugins_dir).map_err(|error| {
        format!(
            "failed to create bundled plugins dir {}: {error}",
            plugins_dir.display()
        )
    })?;

    let bundled_server_binary = stage_dir.join(target.binary_filename(SERVER_BINARY_PACKAGE));
    let server_binary_source =
        compiled_binary_path(workspace_root, SERVER_BINARY_PACKAGE, true, Some(target));
    copy_required_file(&server_binary_source, &bundled_server_binary)?;

    let bundled_config = runtime_dir.join("server.toml");
    copy_required_file(config_path, &bundled_config)?;
    if include_example_config {
        copy_required_file(config_path, &runtime_dir.join("server.toml.example"))?;
    }

    for plugin in plugins {
        let source = compiled_dynamic_library_path(
            workspace_root,
            &plugin.cargo_package,
            true,
            Some(target),
        );
        package_plugin_from_source(&source, &plugins_dir, plugin, target.artifact_key.as_str())?;
    }
    Ok(())
}

fn copy_required_file(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("expected built artifact `{}`", source.display()));
    }
    let Some(parent) = destination.parent() else {
        return Err(format!(
            "destination {} did not have a parent directory",
            destination.display()
        ));
    };
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create parent directory {}: {error}",
            parent.display()
        )
    })?;
    fs::copy(source, destination).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn managed_plugin_ids(plugins: &[PluginSpec]) -> BTreeSet<String> {
    plugins
        .iter()
        .map(|plugin| plugin.plugin_id.clone())
        .collect()
}

fn workspace_root() -> Result<PathBuf, String> {
    let mut attempted = Vec::new();
    let candidates = [
        env::current_dir().ok(),
        env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ];

    for candidate in candidates.into_iter().flatten() {
        attempted.push(candidate.display().to_string());
        if let Some(root) = find_workspace_root(&candidate)? {
            return Ok(root);
        }
    }
    Err(format!(
        "failed to locate workspace root from {}",
        attempted.join(", ")
    ))
}

fn find_workspace_root(start: &Path) -> Result<Option<PathBuf>, String> {
    for ancestor in start.ancestors() {
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
            return Ok(Some(ancestor.to_path_buf()));
        }
    }
    Ok(None)
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

fn resolve_release_bundle_config_path(
    workspace_root: &Path,
    explicit_path: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(path) = explicit_path {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace_root.join(path)
        };
        if resolved.is_file() {
            return Ok(resolved);
        }
        return Err(format!("config file {} does not exist", resolved.display()));
    }

    let example_config = workspace_root.join("runtime").join("server.toml.example");
    if example_config.is_file() {
        return Ok(example_config);
    }

    Err(format!(
        "default release bundle config {} does not exist",
        example_config.display()
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
    let raw_ids = document
        .live
        .plugins
        .allowlist
        .or(document.plugins.allowlist)
        .ok_or_else(|| {
            format!(
                "config {} did not define live.plugins.allowlist or plugins.allowlist",
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
    let leaf = dist_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root");
    parent.join(format!(
        ".revycraft-xtask-staging-{}-{}",
        sanitize_token(leaf),
        std::process::id()
    ))
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
    build_package(workspace_root, &plugin.cargo_package, release, None)
}

fn build_plugin_for_target(
    workspace_root: &Path,
    plugin: &PluginSpec,
    target: &BuildTarget,
) -> Result<(), String> {
    build_package(workspace_root, &plugin.cargo_package, true, Some(target))
}

fn build_server_binary(workspace_root: &Path, target: &BuildTarget) -> Result<(), String> {
    build_package(workspace_root, SERVER_BINARY_PACKAGE, true, Some(target))
}

fn build_package(
    workspace_root: &Path,
    package_name: &str,
    release: bool,
    target: Option<&BuildTarget>,
) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace_root)
        .arg("build")
        .arg("-p")
        .arg(package_name);
    if release {
        command.arg("--release");
    }
    if let Some(target) = target {
        command.arg("--target").arg(&target.triple);
    }
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        command.arg("--target-dir").arg(target_dir);
    }
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        return Ok(());
    }

    let target_suffix = target.map_or_else(String::new, |target| {
        format!(" for target `{}`", target.triple)
    });
    Err(format!(
        "cargo build failed while building `{package_name}`{target_suffix}"
    ))
}

fn package_plugin(
    workspace_root: &Path,
    dist_dir: &Path,
    plugin: &PluginSpec,
    release: bool,
) -> Result<(), String> {
    let artifact_key = artifact_key(env::consts::OS, env::consts::ARCH);
    let source =
        compiled_dynamic_library_path(workspace_root, &plugin.cargo_package, release, None);
    package_plugin_from_source(&source, dist_dir, plugin, &artifact_key)
}

fn package_plugin_from_source(
    source: &Path,
    dist_dir: &Path,
    plugin: &PluginSpec,
    artifact_key: &str,
) -> Result<(), String> {
    if !source.is_file() {
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
    fs::copy(source, &staging).map_err(|error| {
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
        "[plugin]\nid = \"{}\"\nkind = \"{}\"\n\n[artifacts]\n\"{}\" = \"{}\"\n",
        plugin.plugin_id, plugin.plugin_kind, artifact_key, packaged_artifact_name
    );
    fs::write(plugin_dir.join("plugin.toml"), manifest).map_err(|error| error.to_string())
}

fn target_dir(workspace_root: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace_root.join("target"), PathBuf::from)
}

fn compiled_artifact_dir(
    workspace_root: &Path,
    release: bool,
    target: Option<&BuildTarget>,
) -> PathBuf {
    let build_profile = if release { "release" } else { "debug" };
    let base = target_dir(workspace_root);
    match target {
        Some(target) => base.join(&target.triple).join(build_profile),
        None => base.join(build_profile),
    }
}

fn compiled_dynamic_library_path(
    workspace_root: &Path,
    package: &str,
    release: bool,
    target: Option<&BuildTarget>,
) -> PathBuf {
    let file_name = target.map_or_else(
        || dynamic_library_filename_for_os(env::consts::OS, package),
        |target| target.dynamic_library_filename(package),
    );
    compiled_artifact_dir(workspace_root, release, target).join(file_name)
}

fn compiled_binary_path(
    workspace_root: &Path,
    package: &str,
    release: bool,
    target: Option<&BuildTarget>,
) -> PathBuf {
    let file_name = target.map_or_else(
        || binary_filename_for_os(env::consts::OS, package),
        |target| target.binary_filename(package),
    );
    compiled_artifact_dir(workspace_root, release, target).join(file_name)
}

fn dynamic_library_filename_for_os(os: &str, package: &str) -> String {
    let crate_name = package.replace('-', "_");
    match os {
        "windows" => format!("{crate_name}.dll"),
        "macos" => format!("lib{crate_name}.dylib"),
        _ => format!("lib{crate_name}.so"),
    }
}

fn binary_filename_for_os(os: &str, name: &str) -> String {
    if os == "windows" {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn artifact_key(os: &str, arch: &str) -> String {
    format!("{os}-{arch}")
}

fn packaged_artifact_name(base_name: &str) -> String {
    packaged_artifact_name_with_tag(base_name, env::var("REVY_PLUGIN_BUILD_TAG").ok())
}

fn packaged_artifact_name_with_tag(base_name: &str, build_tag: Option<String>) -> String {
    match build_tag {
        Some(build_tag) if !build_tag.is_empty() => {
            let sanitized = sanitize_token(&build_tag);
            if let Some((stem, extension)) = base_name.rsplit_once('.') {
                format!("{stem}-{sanitized}.{extension}")
            } else {
                format!("{base_name}-{sanitized}")
            }
        }
        _ => base_name.to_string(),
    }
}

fn sanitize_token(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BuildTarget, PackageArgs, PluginSpec, filter_plugins_by_ids, find_workspace_root,
        package_plugin_from_source, packaged_artifact_name_with_tag, parse_package_args,
        parse_release_bundle_args, plugin_allowlist_from_toml, plugin_spec_from_package_name,
        reconcile_packaged_plugins, resolve_package_config_path, run_release_bundle_jobs,
        stage_release_bundle,
    };
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn plugin_spec_maps_protocol_packages_to_adapter_ids() {
        assert_eq!(
            plugin_spec_from_package_name("mc-plugin-proto-je-340").expect("valid protocol plugin"),
            PluginSpec {
                cargo_package: "mc-plugin-proto-je-340".to_string(),
                plugin_id: "je-340".to_string(),
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
    fn build_target_maps_supported_target_triples() {
        assert_eq!(
            BuildTarget::from_triple("x86_64-unknown-linux-gnu").expect("linux triple"),
            BuildTarget {
                triple: "x86_64-unknown-linux-gnu".to_string(),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                artifact_key: "linux-x86_64".to_string(),
                dylib_ext: "so".to_string(),
                exe_ext: String::new(),
            }
        );
        assert_eq!(
            BuildTarget::from_triple("aarch64-apple-darwin").expect("darwin triple"),
            BuildTarget {
                triple: "aarch64-apple-darwin".to_string(),
                os: "macos".to_string(),
                arch: "aarch64".to_string(),
                artifact_key: "macos-aarch64".to_string(),
                dylib_ext: "dylib".to_string(),
                exe_ext: String::new(),
            }
        );
        assert_eq!(
            BuildTarget::from_triple("x86_64-pc-windows-msvc")
                .expect("windows triple")
                .exe_ext,
            ".exe"
        );
    }

    #[test]
    fn build_target_rejects_unsupported_target_triples() {
        let arch_error = BuildTarget::from_triple("armv7-unknown-linux-gnueabihf")
            .expect_err("unsupported arch should fail");
        assert!(arch_error.contains("unsupported target architecture"));

        let os_error =
            BuildTarget::from_triple("x86_64-unknown-freebsd").expect_err("unsupported os");
        assert!(os_error.contains("unsupported target operating system"));
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
    fn parse_release_bundle_args_defaults_to_example_config() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let runtime_dir = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should be created");
        let example = runtime_dir.join("server.toml.example");
        fs::write(&example, "[live.plugins]\nallowlist = [\"je-5\"]\n")
            .expect("example config should be written");

        let parsed = parse_release_bundle_args(
            temp_dir.path(),
            &[
                "--target".to_string(),
                "x86_64-unknown-linux-gnu".to_string(),
            ],
        )
        .expect("release bundle args should parse");

        assert_eq!(parsed.output_dir, PathBuf::from("dist").join("releases"));
        assert_eq!(parsed.config_path, example);
        assert!(parsed.include_example_config);
        assert_eq!(
            parsed.targets,
            vec![
                BuildTarget::from_triple("x86_64-unknown-linux-gnu")
                    .expect("linux target should parse")
            ]
        );
    }

    #[test]
    fn parse_release_bundle_args_reads_targets_output_dir_and_config() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let runtime_dir = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should be created");
        let custom = runtime_dir.join("bundle.toml");
        fs::write(&custom, "[live.plugins]\nallowlist = [\"je-5\"]\n")
            .expect("custom config should be written");

        let parsed = parse_release_bundle_args(
            temp_dir.path(),
            &[
                "--target".to_string(),
                "x86_64-unknown-linux-gnu".to_string(),
                "--target".to_string(),
                "aarch64-apple-darwin".to_string(),
                "--target".to_string(),
                "x86_64-unknown-linux-gnu".to_string(),
                "--output-dir".to_string(),
                "custom/releases".to_string(),
                "--config".to_string(),
                "runtime/bundle.toml".to_string(),
            ],
        )
        .expect("release bundle args should parse");

        assert_eq!(parsed.output_dir, PathBuf::from("custom/releases"));
        assert_eq!(parsed.config_path, custom);
        assert!(!parsed.include_example_config);
        assert_eq!(
            parsed.targets,
            vec![
                BuildTarget::from_triple("x86_64-unknown-linux-gnu")
                    .expect("linux target should parse"),
                BuildTarget::from_triple("aarch64-apple-darwin")
                    .expect("darwin target should parse"),
            ]
        );
    }

    #[test]
    fn filter_plugins_by_ids_rejects_unknown_sample_plugin() {
        let error = filter_plugins_by_ids(
            vec![PluginSpec {
                cargo_package: "mc-plugin-proto-je-5".to_string(),
                plugin_id: "je-5".to_string(),
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
        fs::write(&active, "[plugins]\nallowlist = [\"je-5\"]\n")
            .expect("active config should be written");
        fs::write(&example, "[plugins]\nallowlist = [\"je-47\"]\n")
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
        fs::write(&example, "[plugins]\nallowlist = [\"je-47\"]\n")
            .expect("example config should be written");

        assert_eq!(
            resolve_package_config_path(temp_dir.path(), None)
                .expect("example config should be used as fallback"),
            example
        );
    }

    #[test]
    fn find_workspace_root_walks_up_from_nested_directory() {
        let temp_dir = tempdir().expect("temp dir should be created");
        fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .expect("workspace manifest should be written");
        let nested = temp_dir.path().join("tools").join("xtask").join("src");
        fs::create_dir_all(&nested).expect("nested dir should be created");

        assert_eq!(
            find_workspace_root(&nested).expect("workspace root lookup should succeed"),
            Some(temp_dir.path().to_path_buf())
        );
    }

    #[test]
    fn plugin_allowlist_from_toml_reads_deduplicated_ids() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let config = temp_dir.path().join("server.toml");
        fs::write(
            &config,
            "[live.plugins]\nallowlist = [\"je-5\", \"je-5\", \"auth-offline\"]\n",
        )
        .expect("config should be written");

        assert_eq!(
            plugin_allowlist_from_toml(&config).expect("allowlist should be parsed successfully"),
            vec!["je-5".to_string(), "auth-offline".to_string()]
        );
    }

    #[test]
    fn plugin_allowlist_from_toml_falls_back_to_legacy_plugins_table() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let config = temp_dir.path().join("server.toml");
        fs::write(&config, "[plugins]\nallowlist = [\"je-47\"]\n")
            .expect("config should be written");

        assert_eq!(
            plugin_allowlist_from_toml(&config).expect("legacy allowlist should be parsed"),
            vec!["je-47".to_string()]
        );
    }

    #[test]
    fn reconcile_packaged_plugins_keeps_third_party_plugins_and_removes_unselected_managed() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        let stage_dir = temp_dir.path().join("runtime").join(".stage");
        fs::create_dir_all(&dist_dir).expect("dist dir should be created");
        fs::create_dir_all(&stage_dir).expect("stage dir should be created");

        let selected_dist_dir = dist_dir.join("je-5");
        fs::create_dir_all(&selected_dist_dir).expect("selected dist dir should be created");
        fs::write(selected_dist_dir.join("stale.txt"), "stale")
            .expect("stale file should be written");

        let removed_dist_dir = dist_dir.join("je-47");
        fs::create_dir_all(&removed_dist_dir).expect("removed dist dir should be created");
        fs::write(removed_dist_dir.join("plugin.toml"), "old")
            .expect("old manifest should be written");

        let third_party_dir = dist_dir.join("third-party");
        fs::create_dir_all(&third_party_dir).expect("third-party dir should be created");
        fs::write(third_party_dir.join("plugin.toml"), "external")
            .expect("external manifest should be written");

        let staged_plugin_dir = stage_dir.join("je-5");
        fs::create_dir_all(&staged_plugin_dir).expect("staged plugin dir should be created");
        fs::write(staged_plugin_dir.join("plugin.toml"), "new")
            .expect("new manifest should be written");

        reconcile_packaged_plugins(
            &dist_dir,
            &stage_dir,
            &["je-5".to_string(), "je-47".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            &["je-5".to_string()].into_iter().collect::<BTreeSet<_>>(),
        )
        .expect("reconcile should succeed");

        assert_eq!(
            fs::read_to_string(dist_dir.join("je-5").join("plugin.toml"))
                .expect("selected plugin manifest should exist"),
            "new"
        );
        assert!(
            !dist_dir.join("je-5").join("stale.txt").exists(),
            "selected managed plugin should be fully replaced"
        );
        assert!(
            !dist_dir.join("je-47").exists(),
            "unselected managed plugin should be removed"
        );
        assert!(
            dist_dir.join("third-party").exists(),
            "third-party plugin dirs should be preserved"
        );
    }

    #[test]
    fn stage_release_bundle_uses_target_release_artifacts_and_example_layout() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let workspace_root = temp_dir.path();
        let runtime_dir = workspace_root.join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should exist");
        let config_path = runtime_dir.join("server.toml.example");
        fs::write(
            &config_path,
            "[static.plugins]\nplugins_dir = \"plugins\"\n\n[live.plugins]\nallowlist = [\"je-5\"]\n",
        )
        .expect("config should be written");

        let target = BuildTarget::from_triple("x86_64-unknown-linux-gnu")
            .expect("linux target should parse");
        let build_dir = workspace_root
            .join("target")
            .join(&target.triple)
            .join("release");
        fs::create_dir_all(&build_dir).expect("release dir should be created");
        fs::write(build_dir.join("server-bootstrap"), "server-binary")
            .expect("server binary should be written");
        fs::write(
            build_dir.join("libmc_plugin_proto_je_5.so"),
            "plugin-binary",
        )
        .expect("plugin binary should be written");

        let stage_dir = workspace_root.join("dist").join("stage");
        stage_release_bundle(
            workspace_root,
            &stage_dir,
            &target,
            &[PluginSpec {
                cargo_package: "mc-plugin-proto-je-5".to_string(),
                plugin_id: "je-5".to_string(),
                plugin_kind: "protocol".to_string(),
            }],
            &config_path,
            true,
        )
        .expect("bundle staging should succeed");

        assert_eq!(
            fs::read_to_string(stage_dir.join("server-bootstrap"))
                .expect("bundled server binary should exist"),
            "server-binary"
        );
        assert_eq!(
            fs::read_to_string(stage_dir.join("runtime").join("server.toml"))
                .expect("bundled server.toml should exist"),
            fs::read_to_string(&config_path).expect("source config should be readable")
        );
        assert!(
            stage_dir
                .join("runtime")
                .join("server.toml.example")
                .is_file(),
            "default example config should be bundled"
        );
        assert_eq!(
            fs::read_to_string(
                stage_dir
                    .join("runtime")
                    .join("plugins")
                    .join("je-5")
                    .join("libmc_plugin_proto_je_5.so")
            )
            .expect("bundled plugin artifact should exist"),
            "plugin-binary"
        );
        assert_eq!(
            fs::read_to_string(
                stage_dir
                    .join("runtime")
                    .join("plugins")
                    .join("je-5")
                    .join("plugin.toml")
            )
            .expect("bundled plugin manifest should exist"),
            "[plugin]\nid = \"je-5\"\nkind = \"protocol\"\n\n[artifacts]\n\"linux-x86_64\" = \"libmc_plugin_proto_je_5.so\"\n"
        );
    }

    #[test]
    fn stage_release_bundle_failure_preserves_existing_output_on_failure() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let workspace_root = temp_dir.path();
        let runtime_dir = workspace_root.join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir should exist");
        let config_path = runtime_dir.join("server.toml.example");
        fs::write(&config_path, "[live.plugins]\nallowlist = [\"je-5\"]\n")
            .expect("config should be written");

        let target = BuildTarget::from_triple("x86_64-unknown-linux-gnu")
            .expect("linux target should parse");

        let bundle_dir = workspace_root
            .join("dist")
            .join("releases")
            .join(&target.triple);
        fs::create_dir_all(&bundle_dir).expect("existing bundle dir should exist");
        fs::write(bundle_dir.join("keep.txt"), "keep").expect("marker should be written");

        let stage_dir = workspace_root.join("dist").join("stage");
        let error = stage_release_bundle(
            workspace_root,
            &stage_dir,
            &target,
            &[PluginSpec {
                cargo_package: "mc-plugin-proto-je-5".to_string(),
                plugin_id: "je-5".to_string(),
                plugin_kind: "protocol".to_string(),
            }],
            &config_path,
            false,
        )
        .expect_err("missing server binary should fail");
        assert!(error.contains("server-bootstrap"));
        assert_eq!(
            fs::read_to_string(bundle_dir.join("keep.txt"))
                .expect("existing bundle marker should remain"),
            "keep"
        );
    }

    #[test]
    fn run_release_bundle_jobs_keeps_successful_output_when_a_later_target_fails() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let output_root = temp_dir.path().join("dist").join("releases");
        let linux = BuildTarget::from_triple("x86_64-unknown-linux-gnu")
            .expect("linux target should parse");
        let macos =
            BuildTarget::from_triple("aarch64-apple-darwin").expect("darwin target should parse");

        let error = run_release_bundle_jobs(
            &output_root,
            &[linux.clone(), macos.clone()],
            |target, bundle_dir| {
                if target.triple == linux.triple {
                    fs::create_dir_all(bundle_dir).map_err(|err| err.to_string())?;
                    fs::write(bundle_dir.join("ok.txt"), "ok").map_err(|err| err.to_string())?;
                    Ok(())
                } else {
                    Err("missing linker".to_string())
                }
            },
        )
        .expect_err("partial failure should return an error");

        assert!(error.contains(&macos.triple));
        assert_eq!(
            fs::read_to_string(output_root.join(&linux.triple).join("ok.txt"))
                .expect("successful bundle should stay on disk"),
            "ok"
        );
    }

    #[test]
    fn package_plugin_from_source_writes_target_artifact_key() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let source = temp_dir.path().join("mc_plugin.dll");
        fs::write(&source, "dll").expect("plugin dll should be written");

        package_plugin_from_source(
            &source,
            temp_dir.path(),
            &PluginSpec {
                cargo_package: "mc-plugin-auth-offline".to_string(),
                plugin_id: "auth-offline".to_string(),
                plugin_kind: "auth".to_string(),
            },
            "windows-x86_64",
        )
        .expect("packaging should succeed");

        assert_eq!(
            fs::read_to_string(temp_dir.path().join("auth-offline").join("plugin.toml"))
                .expect("manifest should exist"),
            "[plugin]\nid = \"auth-offline\"\nkind = \"auth\"\n\n[artifacts]\n\"windows-x86_64\" = \"mc_plugin.dll\"\n"
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
        if let Some(root) = find_workspace_root(&manifest_dir)
            .expect("workspace root lookup should succeed for xtask tests")
        {
            return root.join("target").join("test-tmp");
        }
        panic!(
            "xtask tests should run under the workspace root: {}",
            manifest_dir.display()
        );
    }
}
