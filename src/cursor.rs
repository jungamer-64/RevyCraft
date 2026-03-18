use bevy::window::{CursorGrabMode, CursorOptions};

pub fn cursor_is_locked(cursor_options: Option<&CursorOptions>) -> bool {
    matches!(
        cursor_options,
        Some(opts) if opts.grab_mode == CursorGrabMode::Locked
    )
}
