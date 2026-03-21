use super::{HashMap, HashSet, RuntimeError};

pub(crate) fn ensure_known_profiles<T>(
    active_profiles: &HashMap<String, T>,
    requested_profiles: &HashSet<String>,
    profile_kind: &str,
) -> Result<(), RuntimeError> {
    for profile_id in requested_profiles {
        ensure_profile_known(active_profiles, profile_id, profile_kind)?;
    }
    Ok(())
}

pub(crate) fn ensure_profile_known<T>(
    active_profiles: &HashMap<String, T>,
    profile_id: &str,
    profile_kind: &str,
) -> Result<(), RuntimeError> {
    if active_profiles.contains_key(profile_id) {
        return Ok(());
    }
    Err(RuntimeError::Config(format!(
        "unknown {profile_kind} profile `{profile_id}`"
    )))
}
