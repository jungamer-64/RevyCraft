use crate::admin_surface::{RustAdminSurfacePlugin, SdkAdminSurfaceHost};
use mc_plugin_api::codec::admin_surface::{AdminSurfaceRequest, AdminSurfaceResponse};
use mc_plugin_api::host_api::AdminSurfaceHostApiV1;

pub fn handle_admin_surface_request<P: RustAdminSurfacePlugin>(
    plugin: &P,
    request: AdminSurfaceRequest,
) -> Result<AdminSurfaceResponse, String> {
    handle_admin_surface_request_with_host_api(plugin, request, None)
}

pub fn handle_admin_surface_request_with_host_api<P: RustAdminSurfacePlugin>(
    plugin: &P,
    request: AdminSurfaceRequest,
    host_api: Option<AdminSurfaceHostApiV1>,
) -> Result<AdminSurfaceResponse, String> {
    let host_api = host_api.ok_or_else(|| "admin-surface host api was unavailable".to_string())?;
    let host = SdkAdminSurfaceHost::new(host_api);
    match request {
        AdminSurfaceRequest::Describe => Ok(AdminSurfaceResponse::Descriptor(plugin.descriptor())),
        AdminSurfaceRequest::CapabilitySet => Ok(AdminSurfaceResponse::CapabilitySet(
            crate::capabilities::admin_surface_announcement(&plugin.capability_set()),
        )),
        AdminSurfaceRequest::DeclareInstance {
            instance_id,
            surface_config_path,
        } => plugin
            .declare_instance(&instance_id, surface_config_path.as_deref())
            .map(AdminSurfaceResponse::Declared),
        AdminSurfaceRequest::Start {
            instance_id,
            surface_config_path,
        } => plugin
            .start(&instance_id, host, surface_config_path.as_deref())
            .map(AdminSurfaceResponse::Started),
        AdminSurfaceRequest::PauseForUpgrade { instance_id } => plugin
            .pause_for_upgrade(&instance_id, host)
            .map(AdminSurfaceResponse::Paused),
        AdminSurfaceRequest::ResumeFromUpgrade {
            instance_id,
            surface_config_path,
            resume_payload,
        } => plugin
            .resume_from_upgrade(
                &instance_id,
                host,
                surface_config_path.as_deref(),
                &resume_payload,
            )
            .map(AdminSurfaceResponse::Resumed),
        AdminSurfaceRequest::ActivateAfterUpgradeCommit { instance_id } => plugin
            .activate_after_upgrade_commit(&instance_id, host)
            .map(|()| AdminSurfaceResponse::Activated),
        AdminSurfaceRequest::ResumeAfterUpgradeRollback { instance_id } => plugin
            .resume_after_upgrade_rollback(&instance_id, host)
            .map(AdminSurfaceResponse::ResumedAfterRollback),
        AdminSurfaceRequest::Shutdown { instance_id } => plugin
            .shutdown(&instance_id, host)
            .map(|()| AdminSurfaceResponse::ShutdownComplete),
    }
}
