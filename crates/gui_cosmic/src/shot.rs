//! Write a window capture as a PNG.

use cosmic::iced::window::Screenshot;
use std::path::Path;

/// Save `shot` (RGBA, physical pixels) to `path`.
pub fn write_png(shot: &Screenshot, path: &Path) -> Result<(), String> {
    image::save_buffer(
        path,
        &shot.rgba,
        shot.size.width,
        shot.size.height,
        image::ColorType::Rgba8,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}
