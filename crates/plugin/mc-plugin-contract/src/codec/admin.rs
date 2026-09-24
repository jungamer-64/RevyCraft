pub use revy_server_types::{
    AdminArtifactsReloadView, AdminCoreReloadView, AdminFullReloadView, AdminGenerationCountView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionTransportCountView,
    AdminSessionView, AdminSessionsView, AdminStatusView, AdminTopologyReloadView,
    AdminUpgradeRuntimeView, CutoverConnectionMix, CutoverOperation, CutoverOutcome, CutoverReport,
    RuntimeReloadMode, RuntimeUpgradePhase, RuntimeUpgradeRole, RuntimeUpgradeStateView,
};

#[cfg(test)]
mod tests {
    use super::{AdminPermission, AdminRequest, AdminResponse, RuntimeReloadMode};

    #[test]
    fn admin_request_json_roundtrip_uses_shared_types() {
        let request = AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Full,
        };
        let encoded = serde_json::to_string(&request).expect("admin request should encode");
        let decoded: AdminRequest =
            serde_json::from_str(&encoded).expect("admin request should decode");
        assert_eq!(decoded, request);
        assert_eq!(
            decoded.required_permission(),
            Some(AdminPermission::ReloadRuntime)
        );
    }

    #[test]
    fn admin_response_json_roundtrip_uses_shared_types() {
        let response = AdminResponse::PermissionDenied {
            principal_id: "ops".to_string(),
            permission: AdminPermission::Shutdown,
        };
        let encoded = serde_json::to_string(&response).expect("admin response should encode");
        let decoded: AdminResponse =
            serde_json::from_str(&encoded).expect("admin response should decode");
        assert_eq!(decoded, response);
    }
}
