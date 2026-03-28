use super::{
    AdminSurfaceProfileId, Arc, AuthProfileId, GameplayProfileId, HotSwappableAdminSurfaceProfile,
    HotSwappableAuthProfile, HotSwappableGameplayProfile, HotSwappableProtocolAdapter,
    HotSwappableStorageProfile, PluginPackage, StorageProfileId, SystemTime,
};

#[derive(Clone)]
pub(crate) struct ManagedProtocolPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) adapter: Arc<HotSwappableProtocolAdapter>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

#[derive(Clone)]
pub(crate) struct ManagedGameplayPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) profile: Arc<HotSwappableGameplayProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

#[derive(Clone)]
pub(crate) struct ManagedStoragePlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: StorageProfileId,
    pub(crate) profile: Arc<HotSwappableStorageProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

#[derive(Clone)]
pub(crate) struct ManagedAuthPlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: AuthProfileId,
    pub(crate) profile: Arc<HotSwappableAuthProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

#[derive(Clone)]
pub(crate) struct ManagedAdminSurfacePlugin {
    pub(crate) package: PluginPackage,
    pub(crate) profile_id: AdminSurfaceProfileId,
    pub(crate) profile: Arc<HotSwappableAdminSurfaceProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}
