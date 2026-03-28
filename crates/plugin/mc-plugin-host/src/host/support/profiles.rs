use super::{HashMap, HashSet, RuntimeError};
use std::fmt::Display;
use std::hash::Hash;

pub(crate) fn ensure_known_profiles<P, T>(
    active_profiles: &HashMap<P, T>,
    requested_profiles: &HashSet<P>,
    profile_kind: &str,
) -> Result<(), RuntimeError>
where
    P: Display + Eq + Hash,
{
    for profile_id in requested_profiles {
        ensure_profile_known(active_profiles, profile_id, profile_kind)?;
    }
    Ok(())
}

pub(crate) fn ensure_profile_known<P, T>(
    active_profiles: &HashMap<P, T>,
    profile_id: &P,
    profile_kind: &str,
) -> Result<(), RuntimeError>
where
    P: Display + Eq + Hash,
{
    if active_profiles.contains_key(profile_id) {
        return Ok(());
    }
    Err(RuntimeError::Config(format!(
        "unknown {profile_kind} profile `{profile_id}`"
    )))
}
