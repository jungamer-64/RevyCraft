use super::{
    Arc, GameplayProfileId, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableProtocolAdapter, HotSwappableStorageProfile, PluginPackage, SystemTime,
};

pub(crate) struct ManagedProtocolPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) adapter: Arc<HotSwappableProtocolAdapter>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedGameplayPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) profile: Arc<HotSwappableGameplayProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedStoragePlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: String,
    pub(crate) profile: Arc<HotSwappableStorageProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedAuthPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: String,
    pub(crate) profile: Arc<HotSwappableAuthProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}
