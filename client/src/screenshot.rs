//! Screenshot save logic.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Save the current frame from FrameSlot to ~/Pictures/rtcdesk/.
///
/// Reads BMP data directly from the shared FrameSlot (written by the
/// decode thread).  Saves as .bmp file — no JPEG re-encoding needed.
///
/// Returns the file path on success.
pub fn save_screenshot() -> Result<String> {
    let slot = crate::get_frame_slot()
        .context("Frame slot not available")?;
    let slot = slot.lock().unwrap();
    let bmp_data = &slot.bmp_data;
    if bmp_data.is_empty() {
        anyhow::bail!("No frame available");
    }

    let dir = dirs::picture_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(shared::frame::SCREENSHOT_DIR);

    std::fs::create_dir_all(&dir)
        .context("创建截图目录失败")?;

    let filename = chrono::Local::now().format("rtcdesk_%Y%m%d_%H%M%S.bmp").to_string();
    let path = dir.join(&filename);

    std::fs::write(&path, bmp_data)
        .context("写入截图文件失败")?;

    Ok(path.display().to_string())
}
