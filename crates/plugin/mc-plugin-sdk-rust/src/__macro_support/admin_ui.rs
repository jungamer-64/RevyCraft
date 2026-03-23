use crate::admin_ui::RustAdminUiPlugin;
use mc_plugin_api::codec::admin_ui::{AdminUiInput, AdminUiOutput};
use mc_plugin_api::host_api::HostApiTableV1;

pub fn handle_admin_ui_request<P: RustAdminUiPlugin>(
    plugin: &P,
    request: AdminUiInput,
) -> Result<AdminUiOutput, String> {
    handle_admin_ui_request_with_host_api(plugin, request, None)
}

pub fn handle_admin_ui_request_with_host_api<P: RustAdminUiPlugin>(
    plugin: &P,
    request: AdminUiInput,
    _host_api: Option<HostApiTableV1>,
) -> Result<AdminUiOutput, String> {
    match request {
        AdminUiInput::Describe => Ok(AdminUiOutput::Descriptor(plugin.descriptor())),
        AdminUiInput::CapabilitySet => Ok(AdminUiOutput::CapabilitySet(
            crate::capabilities::admin_ui_announcement(&plugin.capability_set()),
        )),
        AdminUiInput::ParseLine { line } => {
            plugin.parse_line(&line).map(AdminUiOutput::ParsedRequest)
        }
        AdminUiInput::RenderResponse { response } => plugin
            .render_response(&response)
            .map(AdminUiOutput::RenderedText),
    }
}
