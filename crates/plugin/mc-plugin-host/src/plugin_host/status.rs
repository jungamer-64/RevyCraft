use super::{
    AdminTransportProfileId, AdminUiProfileId, ArtifactIdentity, ArtifactQuarantineRecord,
    AuthMode, AuthProfileId, Deserialize, Edition, GameplayProfileId, PluginBuildTag,
    PluginFailureAction, PluginFailureMatrix, PluginGenerationId, PluginHost, PluginKind,
    Serialize, StorageProfileId, TransportKind, system_time_ms,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginArtifactStatusSnapshot {
    pub source: String,
    pub modified_at_ms: u64,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolPluginStatusSnapshot {
    pub plugin_id: String,
    pub adapter_id: String,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
    pub version_name: String,
    pub transport: TransportKind,
    pub edition: Edition,
    pub protocol_number: i32,
    pub bedrock_listener_descriptor_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameplayPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: GameplayProfileId,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: StorageProfileId,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: AuthProfileId,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
    pub mode: AuthMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: AdminTransportProfileId,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUiPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: AdminUiProfileId,
    pub generation_id: PluginGenerationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostStatusSnapshot {
    pub failure_matrix: PluginFailureMatrix,
    pub pending_fatal_error: Option<String>,
    pub protocols: Vec<ProtocolPluginStatusSnapshot>,
    pub gameplay: Vec<GameplayPluginStatusSnapshot>,
    pub storage: Vec<StoragePluginStatusSnapshot>,
    pub auth: Vec<AuthPluginStatusSnapshot>,
    pub admin_transport: Vec<AdminTransportPluginStatusSnapshot>,
    pub admin_ui: Vec<AdminUiPluginStatusSnapshot>,
}

impl PluginHostStatusSnapshot {
    #[must_use]
    pub fn active_quarantine_count(&self) -> usize {
        self.protocols
            .iter()
            .filter(|plugin| plugin.active_quarantine_reason.is_some())
            .count()
            + self
                .gameplay
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .storage
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .auth
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .admin_transport
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .admin_ui
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
    }

    #[must_use]
    pub fn artifact_quarantine_count(&self) -> usize {
        self.protocols
            .iter()
            .filter(|plugin| plugin.artifact_quarantine.is_some())
            .count()
            + self
                .gameplay
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .storage
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .auth
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .admin_transport
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .admin_ui
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
    }
}

fn artifact_status_snapshot(
    identity: ArtifactIdentity,
    reason: Option<String>,
) -> PluginArtifactStatusSnapshot {
    PluginArtifactStatusSnapshot {
        source: identity.source,
        modified_at_ms: system_time_ms(identity.modified_at),
        reason,
    }
}

fn artifact_quarantine_status_snapshot(
    record: ArtifactQuarantineRecord,
) -> PluginArtifactStatusSnapshot {
    artifact_status_snapshot(record.identity, Some(record.reason))
}

impl PluginHost {
    #[must_use]
    pub fn status(&self) -> PluginHostStatusSnapshot {
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed
                    .adapter
                    .generation
                    .read()
                    .expect("protocol generation lock should not be poisoned")
                    .clone();
                ProtocolPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    adapter_id: generation.descriptor.adapter_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Protocol),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                    version_name: generation.descriptor.version_name.clone(),
                    transport: generation.descriptor.transport,
                    edition: generation.descriptor.edition,
                    protocol_number: generation.descriptor.protocol_number,
                    bedrock_listener_descriptor_present: generation
                        .bedrock_listener_descriptor
                        .is_some(),
                }
            })
            .collect::<Vec<_>>();
        protocols.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed.profile.current_generation();
                GameplayPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Gameplay),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        gameplay.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed.profile.current_generation();
                StoragePluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Storage),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        storage.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed.profile.current_generation();
                AuthPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Auth),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                    mode: generation.mode(),
                }
            })
            .collect::<Vec<_>>();
        auth.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut admin_transport = self
            .admin_transport
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed.profile.current_generation();
                AdminTransportPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::AdminTransport),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        admin_transport.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut admin_ui = self
            .admin_ui
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed.profile.current_generation();
                AdminUiPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    build_tag: generation.build_tag.clone(),
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::AdminUi),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        admin_ui.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        PluginHostStatusSnapshot {
            failure_matrix: self.failures.matrix(),
            pending_fatal_error: self.failures.pending_fatal_message(),
            protocols,
            gameplay,
            storage,
            auth,
            admin_transport,
            admin_ui,
        }
    }
}
