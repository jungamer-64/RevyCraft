use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub fn cursor_is_locked(cursor_options: &Query<&CursorOptions, With<PrimaryWindow>>) -> bool {
    matches!(
        cursor_options.single(),
        Ok(opts) if opts.grab_mode == CursorGrabMode::Locked
    )
}
