use mc_core::{
    AdminTransportCapability, AdminTransportCapabilitySet, AdminUiCapability, AdminUiCapabilitySet,
    AuthCapability, AuthCapabilitySet, CapabilityAnnouncement, GameplayCapability,
    GameplayCapabilitySet, PluginBuildTag, ProtocolCapability, ProtocolCapabilitySet,
    StorageCapability, StorageCapabilitySet,
};

#[must_use]
pub fn protocol_capabilities(capabilities: &[ProtocolCapability]) -> ProtocolCapabilitySet {
    capability_set(capabilities)
}

#[must_use]
pub fn gameplay_capabilities(capabilities: &[GameplayCapability]) -> GameplayCapabilitySet {
    capability_set(capabilities)
}

#[must_use]
pub fn storage_capabilities(capabilities: &[StorageCapability]) -> StorageCapabilitySet {
    capability_set(capabilities)
}

#[must_use]
pub fn auth_capabilities(capabilities: &[AuthCapability]) -> AuthCapabilitySet {
    capability_set(capabilities)
}

#[must_use]
pub fn admin_transport_capabilities(
    capabilities: &[AdminTransportCapability],
) -> AdminTransportCapabilitySet {
    capability_set(capabilities)
}

#[must_use]
pub fn admin_ui_capabilities(capabilities: &[AdminUiCapability]) -> AdminUiCapabilitySet {
    capability_set(capabilities)
}

/// Returns whether the current plugin build tag contains the provided marker.
#[must_use]
pub fn build_tag_contains(needle: &str) -> bool {
    build_tag_contains_in(option_env!("REVY_PLUGIN_BUILD_TAG"), needle)
}

#[must_use]
pub(crate) fn protocol_announcement(
    capabilities: &ProtocolCapabilitySet,
) -> CapabilityAnnouncement<ProtocolCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

#[must_use]
pub(crate) fn gameplay_announcement(
    capabilities: &GameplayCapabilitySet,
) -> CapabilityAnnouncement<GameplayCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

#[must_use]
pub(crate) fn storage_announcement(
    capabilities: &StorageCapabilitySet,
) -> CapabilityAnnouncement<StorageCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

#[must_use]
pub(crate) fn auth_announcement(
    capabilities: &AuthCapabilitySet,
) -> CapabilityAnnouncement<AuthCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

#[must_use]
pub(crate) fn admin_transport_announcement(
    capabilities: &AdminTransportCapabilitySet,
) -> CapabilityAnnouncement<AdminTransportCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

#[must_use]
pub(crate) fn admin_ui_announcement(
    capabilities: &AdminUiCapabilitySet,
) -> CapabilityAnnouncement<AdminUiCapability> {
    capability_announcement_for_build_tag(capabilities, option_env!("REVY_PLUGIN_BUILD_TAG"))
}

fn capability_set<C>(capabilities: &[C]) -> mc_core::ClosedCapabilitySet<C>
where
    C: Copy + Ord,
{
    let mut set = mc_core::ClosedCapabilitySet::new();
    for &capability in capabilities {
        let _ = set.insert(capability);
    }
    set
}

fn capability_announcement_for_build_tag<C>(
    capabilities: &mc_core::ClosedCapabilitySet<C>,
    build_tag: Option<&str>,
) -> CapabilityAnnouncement<C>
where
    C: Copy + Ord,
{
    let mut announced = CapabilityAnnouncement::new(capabilities.clone());
    announced.build_tag = build_tag
        .filter(|tag| !tag.is_empty())
        .map(PluginBuildTag::new);
    announced
}

#[must_use]
pub(crate) fn build_tag_contains_in(build_tag: Option<&str>, needle: &str) -> bool {
    build_tag.is_some_and(|tag| tag.contains(needle))
}
