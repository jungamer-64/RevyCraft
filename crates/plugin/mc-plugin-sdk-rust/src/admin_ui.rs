use super::*;
use mc_core::AdminUiCapabilitySet;

pub trait RustAdminUiPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AdminUiDescriptor;

    fn capability_set(&self) -> AdminUiCapabilitySet {
        AdminUiCapabilitySet::default()
    }

    /// Parses a single operator line into a structured admin request.
    ///
    /// # Errors
    ///
    /// Returns an error when the line cannot be parsed by this UI profile.
    fn parse_line(&self, line: &str) -> Result<AdminRequest, String>;

    /// Renders an admin response into operator-facing text.
    ///
    /// # Errors
    ///
    /// Returns an error when the response cannot be rendered by this UI profile.
    fn render_response(&self, response: &AdminResponse) -> Result<String, String>;
}
