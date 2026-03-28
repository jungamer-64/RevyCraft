use crate::admin_transport::{RustAdminTransportPlugin, SdkAdminTransportHost};
use mc_plugin_api::codec::admin_transport::{AdminTransportRequest, AdminTransportResponse};
use mc_plugin_api::host_api::AdminTransportHostApiV1;

pub fn handle_admin_transport_request<P: RustAdminTransportPlugin>(
    plugin: &P,
    request: AdminTransportRequest,
) -> Result<AdminTransportResponse, String> {
    handle_admin_transport_request_with_host_api(plugin, request, None)
}

pub fn handle_admin_transport_request_with_host_api<P: RustAdminTransportPlugin>(
    plugin: &P,
    request: AdminTransportRequest,
    host_api: Option<AdminTransportHostApiV1>,
) -> Result<AdminTransportResponse, String> {
    let host_api =
        host_api.ok_or_else(|| "admin-transport host api was unavailable".to_string())?;
    let host = SdkAdminTransportHost::new(host_api);
    match request {
        AdminTransportRequest::Describe => {
            Ok(AdminTransportResponse::Descriptor(plugin.descriptor()))
        }
        AdminTransportRequest::CapabilitySet => Ok(AdminTransportResponse::CapabilitySet(
            crate::capabilities::admin_transport_announcement(&plugin.capability_set()),
        )),
        AdminTransportRequest::Start {
            transport_config_path,
        } => plugin
            .start(host, &transport_config_path)
            .map(AdminTransportResponse::Started),
        AdminTransportRequest::PauseForUpgrade => plugin
            .pause_for_upgrade(host)
            .map(AdminTransportResponse::Paused),
        AdminTransportRequest::ResumeFromUpgrade {
            transport_config_path,
            resume_payload,
        } => plugin
            .resume_from_upgrade(host, &transport_config_path, &resume_payload)
            .map(AdminTransportResponse::Resumed),
        AdminTransportRequest::ResumeAfterUpgradeRollback => plugin
            .resume_after_upgrade_rollback(host)
            .map(AdminTransportResponse::ResumedAfterRollback),
        AdminTransportRequest::Shutdown => plugin
            .shutdown(host)
            .map(|()| AdminTransportResponse::ShutdownComplete),
    }
}
