//! 截图保存逻辑。

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::path::PathBuf;

/// 将 base64 编码的 JPEG 帧保存到 ~/Pictures/rtcdesk/。
///
/// `frame_b64` 应为完整的 data URI：`data:image/jpeg;base64,...`
/// 成功时返回文件路径。
pub fn save_screenshot(frame_b64: &str) -> Result<String> {
    // 去掉 data URI 前缀
    let b64_data = if let Some(idx) = frame_b64.find(",") {
        &frame_b64[idx + 1..]
    } else {
        frame_b64
    };

    let jpeg_bytes = BASE64
        .decode(b64_data)
        .context("base64 解码失败")?;

    let dir = dirs::picture_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(shared::frame::SCREENSHOT_DIR);

    std::fs::create_dir_all(&dir)
        .context("创建截图目录失败")?;

    let filename = chrono::Local::now().format("rtcdesk_%Y%m%d_%H%M%S.jpg").to_string();
    let path = dir.join(&filename);

    std::fs::write(&path, &jpeg_bytes)
        .context("写入截图文件失败")?;

    Ok(path.display().to_string())
}
