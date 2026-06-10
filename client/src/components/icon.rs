use std::sync::LazyLock;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

pub fn load_tray_icon() -> Option<tray::Icon> {
    let bytes = include_bytes!("../assets/favicon.webp");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    tray::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

pub fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let bytes = include_bytes!("../assets/favicon.webp");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

const LOGO_BYTES: &[u8] = include_bytes!("../assets/favicon.webp");
pub static LOGO_DATA_URL: LazyLock<String> = LazyLock::new(|| {
    format!("data:image/webp;base64,{}", STANDARD.encode(LOGO_BYTES))
});
