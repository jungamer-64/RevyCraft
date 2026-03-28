mod support;

#[path = "upgrade_runtime/common.rs"]
mod common;
#[cfg(unix)]
#[path = "upgrade_runtime/console_cases.rs"]
mod console_cases;
#[path = "upgrade_runtime/failure_cases.rs"]
mod failure_cases;
#[path = "upgrade_runtime/lock.rs"]
mod lock;
#[path = "upgrade_runtime/options.rs"]
mod options;
#[path = "upgrade_runtime/permission_cases.rs"]
mod permission_cases;
#[path = "upgrade_runtime/sessions.rs"]
mod sessions;
#[path = "upgrade_runtime/success_freeze_cases.rs"]
mod success_freeze_cases;
#[path = "upgrade_runtime/success_online_cases.rs"]
mod success_online_cases;
#[path = "upgrade_runtime/success_play_cases.rs"]
mod success_play_cases;
#[path = "upgrade_runtime/success_status_cases.rs"]
mod success_status_cases;
#[path = "upgrade_runtime/success_support.rs"]
mod success_support;
