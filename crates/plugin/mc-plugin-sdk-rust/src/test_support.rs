use mc_plugin_api::codec::admin_surface::{AdminSurfaceRequest, AdminSurfaceResponse};
use mc_plugin_api::codec::auth::{AuthRequest, AuthResponse};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse};
use mc_plugin_api::codec::storage::{StorageRequest, StorageResponse};
use mc_plugin_api::host_api::{AdminSurfaceHostApiV1, GameplayHostApiV2};
use mc_plugin_api::manifest::PluginManifestV1;

pub trait ProtocolPluginHandler: Send + Sync + 'static {
    fn handle(&self, request: ProtocolRequest) -> Result<ProtocolResponse, String>;
}

pub trait GameplayPluginHandler: Send + Sync + 'static {
    fn handle(
        &self,
        request: GameplayRequest,
        host_api: Option<GameplayHostApiV2>,
    ) -> Result<GameplayResponse, String>;
}

pub trait StoragePluginHandler: Send + Sync + 'static {
    fn handle(&self, request: StorageRequest) -> Result<StorageResponse, String>;
}

pub trait AuthPluginHandler: Send + Sync + 'static {
    fn handle(&self, request: AuthRequest) -> Result<AuthResponse, String>;
}

pub trait AdminSurfacePluginHandler: Send + Sync + 'static {
    fn handle(
        &self,
        request: AdminSurfaceRequest,
        host_api: Option<AdminSurfaceHostApiV1>,
    ) -> Result<AdminSurfaceResponse, String>;
}

pub type ProtocolPluginFactory = fn() -> Box<dyn ProtocolPluginHandler>;
pub type GameplayPluginFactory = fn() -> Box<dyn GameplayPluginHandler>;
pub type StoragePluginFactory = fn() -> Box<dyn StoragePluginHandler>;
pub type AuthPluginFactory = fn() -> Box<dyn AuthPluginHandler>;
pub type AdminSurfacePluginFactory = fn() -> Box<dyn AdminSurfacePluginHandler>;

#[derive(Clone, Copy)]
pub struct InProcessProtocolPluginEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub factory: ProtocolPluginFactory,
}

impl InProcessProtocolPluginEntrypoints {
    #[must_use]
    pub const fn new(manifest: &'static PluginManifestV1, factory: ProtocolPluginFactory) -> Self {
        Self { manifest, factory }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessGameplayPluginEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub factory: GameplayPluginFactory,
}

impl InProcessGameplayPluginEntrypoints {
    #[must_use]
    pub const fn new(manifest: &'static PluginManifestV1, factory: GameplayPluginFactory) -> Self {
        Self { manifest, factory }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessStoragePluginEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub factory: StoragePluginFactory,
}

impl InProcessStoragePluginEntrypoints {
    #[must_use]
    pub const fn new(manifest: &'static PluginManifestV1, factory: StoragePluginFactory) -> Self {
        Self { manifest, factory }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessAuthPluginEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub factory: AuthPluginFactory,
}

impl InProcessAuthPluginEntrypoints {
    #[must_use]
    pub const fn new(manifest: &'static PluginManifestV1, factory: AuthPluginFactory) -> Self {
        Self { manifest, factory }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessAdminSurfacePluginEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub factory: AdminSurfacePluginFactory,
}

impl InProcessAdminSurfacePluginEntrypoints {
    #[must_use]
    pub const fn new(
        manifest: &'static PluginManifestV1,
        factory: AdminSurfacePluginFactory,
    ) -> Self {
        Self { manifest, factory }
    }
}
