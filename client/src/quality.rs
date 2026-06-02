//! Quality preset definitions shared across UI components.

/// `(jpeg_quality, label, width, height, fps, bitrate_kbps)`
#[rustfmt::skip]
pub const QUALITY_PRESETS: &[(u32, &str, u32, u32, u32, u32)] = &[
    (70, "最佳画质", 1920, 1080, 30, 10_000),
    (50, "均衡",     1280, 720,  25, 5_000),
    (30, "流畅优先", 1024, 576,  20, 2_000),
    (10, "低带宽",    800, 450,  15, 500),
];

/// Scale mode for the remote viewport.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScaleMode {
    Fit,
    Stretch,
    Original,
}

impl ScaleMode {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::Fit      => "自适应",
            ScaleMode::Stretch  => "拉伸填充",
            ScaleMode::Original => "原始大小",
        }
    }
}

pub const SCALE_MODES: &[ScaleMode] = &[ScaleMode::Fit, ScaleMode::Stretch, ScaleMode::Original];
