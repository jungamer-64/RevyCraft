use super::{
    ArtifactIdentity, Deserialize, HashMap, Mutex, PluginKind, RuntimeError, Serialize,
    system_time_ms,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFailureAction {
    Quarantine,
    Skip,
    FailFast,
}

impl PluginFailureAction {
    fn parse_with_allowed(value: &str, key: &str, allowed: &[Self]) -> Result<Self, RuntimeError> {
        let action = if value.eq_ignore_ascii_case("quarantine") {
            Self::Quarantine
        } else if value.eq_ignore_ascii_case("skip") {
            Self::Skip
        } else if value.eq_ignore_ascii_case("fail-fast") {
            Self::FailFast
        } else {
            return Err(RuntimeError::Config(format!("unsupported {key} `{value}`")));
        };
        if allowed.contains(&action) {
            Ok(action)
        } else {
            Err(RuntimeError::Config(format!("unsupported {key} `{value}`")))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFailureMatrix {
    pub protocol: PluginFailureAction,
    pub gameplay: PluginFailureAction,
    pub storage: PluginFailureAction,
    pub auth: PluginFailureAction,
    pub admin_transport: PluginFailureAction,
    pub admin_ui: PluginFailureAction,
}

impl Default for PluginFailureMatrix {
    fn default() -> Self {
        Self {
            protocol: PluginFailureAction::Quarantine,
            gameplay: PluginFailureAction::Quarantine,
            storage: PluginFailureAction::FailFast,
            auth: PluginFailureAction::Skip,
            admin_transport: PluginFailureAction::Skip,
            admin_ui: PluginFailureAction::Skip,
        }
    }
}

impl PluginFailureMatrix {
    pub fn parse_protocol(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-protocol",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub fn parse_gameplay(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-gameplay",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub fn parse_storage(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-storage",
            &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
        )
    }

    pub fn parse_auth(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-auth",
            &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
        )
    }

    pub fn parse_admin_transport(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-admin-transport",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub fn parse_admin_ui(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-admin-ui",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub(crate) const fn action_for_kind(self, kind: PluginKind) -> PluginFailureAction {
        match kind {
            PluginKind::Protocol => self.protocol,
            PluginKind::Gameplay => self.gameplay,
            PluginKind::Storage => self.storage,
            PluginKind::Auth => self.auth,
            PluginKind::AdminTransport => self.admin_transport,
            PluginKind::AdminUi => self.admin_ui,
        }
    }
}

#[derive(Default)]
pub(crate) struct ActiveQuarantineManager {
    reasons: Mutex<HashMap<String, String>>,
}

impl ActiveQuarantineManager {
    fn quarantine(&self, plugin_id: &str, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        let mut reasons = self
            .reasons
            .lock()
            .expect("quarantine mutex should not be poisoned");
        let changed = reasons.get(plugin_id) != Some(&reason);
        reasons.insert(plugin_id.to_string(), reason);
        changed
    }

    fn is_quarantined(&self, plugin_id: &str) -> bool {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .contains_key(plugin_id)
    }

    fn clear(&self, plugin_id: &str) {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .remove(plugin_id);
    }

    fn reason(&self, plugin_id: &str) -> Option<String> {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .get(plugin_id)
            .cloned()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactQuarantineRecord {
    pub(crate) identity: ArtifactIdentity,
    pub(crate) reason: String,
}

#[derive(Default)]
pub(crate) struct ArtifactQuarantineManager {
    records: Mutex<HashMap<String, ArtifactQuarantineRecord>>,
}

impl ArtifactQuarantineManager {
    fn quarantine(
        &self,
        plugin_id: &str,
        identity: ArtifactIdentity,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        let mut records = self
            .records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned");
        let changed = records
            .get(plugin_id)
            .is_none_or(|record| record.identity != identity || record.reason != reason);
        records.insert(
            plugin_id.to_string(),
            ArtifactQuarantineRecord { identity, reason },
        );
        changed
    }

    fn is_quarantined(&self, plugin_id: &str, identity: &ArtifactIdentity) -> bool {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .is_some_and(|record| &record.identity == identity)
    }

    fn clear(&self, plugin_id: &str) {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .remove(plugin_id);
    }

    fn reason(&self, plugin_id: &str, identity: &ArtifactIdentity) -> Option<String> {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .filter(|record| &record.identity == identity)
            .map(|record| record.reason.clone())
    }

    fn record(&self, plugin_id: &str) -> Option<ArtifactQuarantineRecord> {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .cloned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginFailureStage {
    Boot,
    Reload,
    Runtime,
}

impl PluginFailureStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Reload => "reload",
            Self::Runtime => "runtime",
        }
    }
}

pub(crate) struct PluginFailureDispatch {
    matrix: Mutex<PluginFailureMatrix>,
    active_quarantine: ActiveQuarantineManager,
    artifact_quarantine: ArtifactQuarantineManager,
    pending_fatal: Mutex<Option<String>>,
}

impl PluginFailureDispatch {
    pub(crate) fn new(matrix: PluginFailureMatrix) -> Self {
        Self {
            matrix: Mutex::new(matrix),
            active_quarantine: ActiveQuarantineManager {
                reasons: Mutex::new(HashMap::new()),
            },
            artifact_quarantine: ArtifactQuarantineManager {
                records: Mutex::new(HashMap::new()),
            },
            pending_fatal: Mutex::new(None),
        }
    }

    pub(crate) fn update_matrix(&self, matrix: PluginFailureMatrix) {
        *self
            .matrix
            .lock()
            .expect("failure matrix mutex should not be poisoned") = matrix;
    }

    pub(crate) fn action_for_kind(&self, kind: PluginKind) -> PluginFailureAction {
        self.matrix
            .lock()
            .expect("failure matrix mutex should not be poisoned")
            .action_for_kind(kind)
    }

    pub(crate) fn matrix(&self) -> PluginFailureMatrix {
        *self
            .matrix
            .lock()
            .expect("failure matrix mutex should not be poisoned")
    }

    pub(crate) fn active_reason(&self, plugin_id: &str) -> Option<String> {
        self.active_quarantine.reason(plugin_id)
    }

    pub(crate) fn pending_fatal_message(&self) -> Option<String> {
        self.pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned")
            .clone()
    }

    pub(crate) fn is_active_quarantined(&self, plugin_id: &str) -> bool {
        self.active_quarantine.is_quarantined(plugin_id)
    }

    pub(crate) fn is_artifact_quarantined(
        &self,
        plugin_id: &str,
        identity: &ArtifactIdentity,
    ) -> bool {
        self.artifact_quarantine.is_quarantined(plugin_id, identity)
    }

    pub(crate) fn artifact_reason(
        &self,
        plugin_id: &str,
        identity: &ArtifactIdentity,
    ) -> Option<String> {
        self.artifact_quarantine.reason(plugin_id, identity)
    }

    pub(crate) fn artifact_record(&self, plugin_id: &str) -> Option<ArtifactQuarantineRecord> {
        self.artifact_quarantine.record(plugin_id)
    }

    pub(crate) fn clear_plugin_state(&self, plugin_id: &str) {
        self.active_quarantine.clear(plugin_id);
        self.artifact_quarantine.clear(plugin_id);
    }

    fn record_fatal_message(&self, message: String) {
        let mut pending = self
            .pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned");
        if pending.is_none() {
            eprintln!("plugin fail-fast scheduled graceful shutdown: {message}");
            *pending = Some(message);
        }
    }

    const fn kind_label(kind: PluginKind) -> &'static str {
        match kind {
            PluginKind::Protocol => "protocol",
            PluginKind::Gameplay => "gameplay",
            PluginKind::Storage => "storage",
            PluginKind::Auth => "auth",
            PluginKind::AdminTransport => "admin-transport",
            PluginKind::AdminUi => "admin-ui",
        }
    }

    pub(crate) fn take_pending_fatal_error(&self) -> Option<RuntimeError> {
        self.pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned")
            .take()
            .map(RuntimeError::PluginFatal)
    }

    fn fail_fast_message(
        kind: PluginKind,
        stage: PluginFailureStage,
        plugin_id: &str,
        reason: &str,
    ) -> String {
        format!(
            "{} plugin `{plugin_id}` failed during {}: {reason}",
            match kind {
                PluginKind::Protocol => "protocol",
                PluginKind::Gameplay => "gameplay",
                PluginKind::Storage => "storage",
                PluginKind::Auth => "auth",
                PluginKind::AdminTransport => "admin-transport",
                PluginKind::AdminUi => "admin-ui",
            },
            stage.as_str(),
        )
    }

    pub(crate) fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction {
        let action = self.action_for_kind(kind);
        match action {
            PluginFailureAction::Quarantine => {
                if self
                    .active_quarantine
                    .quarantine(plugin_id, reason.to_string())
                {
                    eprintln!(
                        "{} plugin `{plugin_id}` entered active quarantine: {reason}",
                        Self::kind_label(kind)
                    );
                }
            }
            PluginFailureAction::FailFast => {
                self.record_fatal_message(Self::fail_fast_message(
                    kind,
                    PluginFailureStage::Runtime,
                    plugin_id,
                    reason,
                ));
            }
            PluginFailureAction::Skip => {}
        }
        action
    }

    pub(crate) fn handle_candidate_failure(
        &self,
        kind: PluginKind,
        stage: PluginFailureStage,
        plugin_id: &str,
        identity: ArtifactIdentity,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        match self.action_for_kind(kind) {
            PluginFailureAction::Skip => Ok(()),
            PluginFailureAction::Quarantine => {
                let modified_at_ms = system_time_ms(identity.modified_at);
                let source = identity.source.clone();
                if self
                    .artifact_quarantine
                    .quarantine(plugin_id, identity, reason.to_string())
                {
                    eprintln!(
                        "{} plugin `{plugin_id}` artifact quarantined during {}: source={} modified_at_ms={} reason={reason}",
                        Self::kind_label(kind),
                        stage.as_str(),
                        source,
                        modified_at_ms,
                    );
                }
                Ok(())
            }
            PluginFailureAction::FailFast => {
                let message = Self::fail_fast_message(kind, stage, plugin_id, reason);
                self.record_fatal_message(message.clone());
                Err(RuntimeError::PluginFatal(message))
            }
        }
    }

    pub(crate) fn quarantine_candidate_artifact(
        &self,
        kind: PluginKind,
        stage: PluginFailureStage,
        plugin_id: &str,
        identity: ArtifactIdentity,
        reason: &str,
    ) {
        let modified_at_ms = system_time_ms(identity.modified_at);
        let source = identity.source.clone();
        if self
            .artifact_quarantine
            .quarantine(plugin_id, identity, reason.to_string())
        {
            eprintln!(
                "{} plugin `{plugin_id}` artifact quarantined during {}: source={} modified_at_ms={} reason={reason}",
                Self::kind_label(kind),
                stage.as_str(),
                source,
                modified_at_ms,
            );
        }
    }
}
