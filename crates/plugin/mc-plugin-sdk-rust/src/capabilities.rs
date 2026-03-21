use mc_core::CapabilitySet;

/// Builds a capability set for the current plugin build.
#[must_use]
pub fn capability_set(names: &[&str]) -> CapabilitySet {
    capability_set_for_build_tag(names, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

/// Returns whether the current plugin build tag contains the provided marker.
#[must_use]
pub fn build_tag_contains(needle: &str) -> bool {
    build_tag_contains_in(option_env!("REVY_PLUGIN_BUILD_TAG"), needle)
}

#[must_use]
pub(crate) fn capability_set_for_build_tag(
    names: &[&str],
    build_tag: Option<&str>,
) -> CapabilitySet {
    let mut capabilities = CapabilitySet::new();
    for &name in names {
        let _ = capabilities.insert(name);
    }
    if let Some(build_tag) = build_tag.filter(|tag| !tag.is_empty()) {
        let _ = capabilities.insert(format!("build-tag:{build_tag}"));
    }
    capabilities
}

#[must_use]
pub(crate) fn build_tag_contains_in(build_tag: Option<&str>, needle: &str) -> bool {
    build_tag.is_some_and(|tag| tag.contains(needle))
}
