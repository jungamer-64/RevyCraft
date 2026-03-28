use crate::support::{ServerTomlOptions, UPGRADE_CONSOLE_PERMISSIONS, UPGRADE_REMOTE_PERMISSIONS};

pub(crate) fn remote_admin_upgrade_options(
    grpc_port: u16,
    motd: &'static str,
) -> ServerTomlOptions<'static> {
    let mut options = ServerTomlOptions::new(true, 0, grpc_port, motd);
    options.console_permissions = UPGRADE_CONSOLE_PERMISSIONS;
    options.remote_permissions = UPGRADE_REMOTE_PERMISSIONS;
    options
}
