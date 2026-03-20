use crate::RuntimeError;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(test)]
pub fn packaged_plugin_test_build_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
pub fn packaged_plugin_test_workspace_root() -> PathBuf {
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
            return ancestor.to_path_buf();
        }
    }
    panic!(
        "server-runtime crate should live under the workspace root: {}",
        manifest_dir.display()
    );
}

#[cfg(test)]
pub fn packaged_plugin_test_target_dir(_scope: &str) -> PathBuf {
    // Reuse one cargo target directory across packaged-plugin tests so cargo can
    // keep dependency and plugin builds incremental across test cases and reruns.
    packaged_plugin_test_workspace_root()
        .join("target")
        .join("server-runtime-packaged-plugin-builds")
        .join("cargo-target")
}

#[cfg(test)]
static PACKAGED_PLUGIN_TEST_HARNESS_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static PACKAGED_PLUGIN_TEST_HARNESS: std::sync::OnceLock<Result<PathBuf, String>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";

#[cfg(test)]
pub fn packaged_plugin_test_harness_build_count() -> usize {
    PACKAGED_PLUGIN_TEST_HARNESS_BUILDS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
fn packaged_plugin_test_run_xtask_package_plugins(
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), String> {
    let _guard = packaged_plugin_test_build_lock()
        .lock()
        .expect("packaged plugin build lock should not be poisoned");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
    let status = std::process::Command::new(cargo)
        .current_dir(packaged_plugin_test_workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("run")
        .arg("-p")
        .arg("xtask")
        .arg("--")
        .arg("package-plugins")
        .arg("--dist-dir")
        .arg(dist_dir)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("xtask package-plugins failed".to_string())
    }
}

#[cfg(test)]
fn packaged_plugin_test_harness_stamp() -> Result<String, String> {
    let mut newest_ms = 0_u128;
    let mut file_count = 0_u64;
    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        "tools/xtask",
        "plugins",
        "crates/core",
        "crates/plugin",
        "crates/protocol",
    ] {
        accumulate_packaged_plugin_test_stamp(
            &packaged_plugin_test_workspace_root().join(relative),
            &mut newest_ms,
            &mut file_count,
        )?;
    }
    Ok(format!("{newest_ms}-{file_count}"))
}

#[cfg(test)]
fn accumulate_packaged_plugin_test_stamp(
    path: &Path,
    newest_ms: &mut u128,
    file_count: &mut u64,
) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "failed to inspect packaged plugin test input {}: {error}",
            path.display()
        )
    })?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| {
            format!(
                "failed to read packaged plugin test input directory {}: {error}",
                path.display()
            )
        })? {
            let entry = entry.map_err(|error| error.to_string())?;
            accumulate_packaged_plugin_test_stamp(&entry.path(), newest_ms, file_count)?;
        }
        return Ok(());
    }

    *file_count += 1;
    let modified_ms = metadata
        .modified()
        .map_err(|error| {
            format!(
                "failed to read modification time for {}: {error}",
                path.display()
            )
        })?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            format!(
                "modification time for {} predates unix epoch: {error}",
                path.display()
            )
        })?
        .as_millis();
    *newest_ms = (*newest_ms).max(modified_ms);
    Ok(())
}

#[cfg(test)]
pub fn packaged_plugin_test_harness_dist_dir() -> Result<&'static PathBuf, String> {
    match PACKAGED_PLUGIN_TEST_HARNESS
        .get_or_init(|| {
            PACKAGED_PLUGIN_TEST_HARNESS_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root_dir = packaged_plugin_test_workspace_root()
                .join("target")
                .join("server-runtime-plugin-test-harness");
            let dist_dir = root_dir.join("runtime").join("plugins");
            let stamp_path = root_dir.join("stamp.txt");
            let stamp = packaged_plugin_test_harness_stamp()?;
            if dist_dir.is_dir()
                && stamp_path.is_file()
                && fs::read_to_string(&stamp_path)
                    .map(|current| current == stamp)
                    .unwrap_or(false)
            {
                return Ok(dist_dir);
            }
            if root_dir.exists() {
                fs::remove_dir_all(&root_dir).map_err(|error| error.to_string())?;
            }
            let target_dir = packaged_plugin_test_target_dir("runtime-test-harness");
            fs::create_dir_all(&dist_dir).map_err(|error| error.to_string())?;
            packaged_plugin_test_run_xtask_package_plugins(
                &dist_dir,
                &target_dir,
                PACKAGED_PLUGIN_TEST_HARNESS_TAG,
            )?;
            fs::write(&stamp_path, stamp).map_err(|error| error.to_string())?;
            Ok(dist_dir)
        })
        .as_ref()
    {
        Ok(dist_dir) => Ok(dist_dir),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
fn copy_packaged_plugin_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry_type.is_dir() {
            copy_packaged_plugin_tree(&source_path, &destination_path)?;
            continue;
        }
        if destination_path.exists() {
            fs::remove_file(&destination_path)?;
        }
        if fs::hard_link(&source_path, &destination_path).is_err() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn seed_packaged_plugins_from_test_harness(dist_dir: &Path) -> Result<(), RuntimeError> {
    let harness_dist_dir = packaged_plugin_test_harness_dist_dir().map_err(RuntimeError::Config)?;
    if dist_dir.exists() {
        fs::remove_dir_all(dist_dir)?;
    }
    copy_packaged_plugin_tree(harness_dist_dir, dist_dir)?;
    Ok(())
}
