use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub fn cursor_is_locked(cursor_options: &Query<&CursorOptions, With<PrimaryWindow>>) -> bool {
    matches!(
        cursor_options.single(),
        Ok(cursor_options) if cursor_options.grab_mode == CursorGrabMode::Locked
    )
}
