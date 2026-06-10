//! RTCDesk Desktop GUI Client — entry point.
//!
//! RustDesk-style Dioxus UI. Code is split into the following modules:
//!
//! - `config`       — AppConfig, load/save, STUN server list
//! - `config`       — AppConfig, load/save, STUN server list
//! - `local_id`     — device ID generation and formatting
//! - `quality`      — QUALITY_PRESETS, ScaleMode
//! - `clipboard`    — system clipboard get/set
//! - `components/`  — titlebar, sidebar, home, connect, settings
//! - `remote/`      — view, toolbar, keyboard, mouse, viewer_loop

mod clipboard;
mod components;
mod config;
mod decoder;
mod frame_slot;
mod host_process;
mod i18n;
mod quality;
mod remote;
mod screenshot;
mod theme;
mod toast;
mod viewer;

use crate::components::icon::{load_tray_icon, load_window_icon};
use crate::frame_slot::{FrameSlot, SharedFrameSlot, FRAME_PROTOCOL_NAME};
use dioxus::desktop::use_window;
use dioxus::desktop::{self, tao::window::Fullscreen};
use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
use std::sync::{Arc, Mutex};
use dioxus::core::spawn_isomorphic;
use dioxus::desktop::tao::window::Window;

/// Global frame slot, set once at startup, read by RemoteView.
static FRAME_SLOT_GLOBAL: std::sync::Mutex<Option<Arc<SharedFrameSlot>>> =
    std::sync::Mutex::new(None);

/// Get the global frame slot (called from RemoteView).
pub fn get_frame_slot() -> Option<Arc<SharedFrameSlot>> {
    FRAME_SLOT_GLOBAL.lock().unwrap().clone()
}

use shared::local_id;

use crate::components::auth::AuthCtx;
use crate::components::connect::ConnectTab;
use crate::components::home::HomeTab;
use crate::components::host::HostCtx;
use crate::components::host::HostState;
use crate::components::host::HostTab;
use crate::components::resize::ResizeEdges;
use crate::components::settings::SettingsTab;
use crate::components::sidebar::{NavTab, Sidebar};
use crate::components::titlebar::TitleBar;
use crate::config::ThemeConfig;
use crate::config::{load_config, load_stun_servers, save_config};
use crate::remote::view::RemoteView;
use dioxus_bootstrap_css::theme::Theme;
use tray::{MouseButton, MouseButtonState, TrayIconEvent};
// ─── Page type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    Main,
    Remote,
}

// ─── Window command (message-driven window control) ────────────────────────
//
// All window operations (fullscreen, minimize, maximize, restore) are expressed
// as commands.  Only the root `app()` component holds a `DesktopService` handle
// and interprets these commands — child components never touch the window
// directly; they just send commands via the global signal.

#[derive(Debug, Clone, PartialEq)]
pub enum WindowCmd {
    /// Toggle borderless-fullscreen (remote desktop mode).
    ToggleFullscreen,
    /// Exit fullscreen unconditionally (used on disconnect).
    ExitFullscreen,
    /// Minimize the window.
    Minimize,
    /// Toggle maximized / restored.
    ToggleMaximize,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let win_builder = desktop::WindowBuilder::new()
        .with_title("RTCDesk")
        .with_window_icon(load_window_icon())
        .with_decorations(false)
        .with_resizable(true);

    // Shared frame slot for the custom rtcdesk:// protocol
    let frame_slot: Arc<SharedFrameSlot> = Arc::new(Mutex::new(FrameSlot::new()));
    let frame_slot_protocol = frame_slot.clone();

    let cfg = desktop::Config::new()
        .with_window(win_builder)
        .with_close_behaviour(desktop::WindowCloseBehaviour::WindowHides)
        .with_custom_protocol(FRAME_PROTOCOL_NAME, move |_webview_id, _request| {
            let slot = frame_slot_protocol.lock().unwrap();
            if slot.bmp_data.is_empty() {
                desktop::wry::http::Response::builder()
                    .status(404)
                    .body(std::borrow::Cow::Borrowed(&[] as &[u8]))
                    .unwrap()
            } else {
                desktop::wry::http::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/bmp")
                    .header("Cache-Control", "no-cache, no-store")
                    .body(std::borrow::Cow::Owned(slot.bmp_data.clone()))
                    .unwrap()
            }
        });

    // Store frame_slot in a global so RemoteView can access it
    *FRAME_SLOT_GLOBAL.lock().unwrap() = Some(frame_slot);

    dioxus::LaunchBuilder::new().with_cfg(cfg).launch(app);
}

// ─── Root component ───────────────────────────────────────────────────────────

#[component]
fn app() -> Element {
    // Inject custom app.css (minimal — only window chrome, remote session, theme vars)
    let mut css_injected = use_signal(|| false);
    use_effect(move || {
        if css_injected() {
            return;
        }
        css_injected.set(true);
        let css = include_str!("assets/app.css");
        let escaped = css
            .replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace('$', "\\$");
        let _ = document::eval(&format!(
            "var s = document.createElement('style'); s.textContent = `{}`; document.head.appendChild(s);",
            escaped
        ));
    });

    let cfg = load_config();
    let page = use_signal(|| Page::Main);
    let server_addr = use_signal(move || cfg.server_addr.clone());
    let remote_id = use_signal(move || cfg.remote_id.clone());
    let quality = use_signal(move || cfg.quality);
    let theme = use_signal(move || cfg.theme.0);
    let locale = use_signal(move || cfg.locale.clone());
    let local_id = use_signal(move || cfg.local_id.clone());
    let password = use_signal(String::new);

    // Persist theme to config on change
    use_effect(move || {
        let t = theme();
        let mut c = load_config();
        let tc = ThemeConfig(t);
        if c.theme.0 != t {
            c.theme = tc;
            save_config(&c);
        }
    });

    // Provide shared Host context (persists across tab switches)
    use_context_provider(|| HostCtx::new());

    // Provide shared Auth context (persists across tab switches)
    let auth_ctx = AuthCtx::new();
    use_context_provider(|| auth_ctx.clone());

    // ── Window command bus ─────────────────────────────────────────────────
    // Global queue for window operations.  Only `app()` holds the DesktopService
    // handle and executes commands; all child components push commands here.
    let mut window_cmd: Signal<Vec<WindowCmd>> = use_signal(Vec::new);
    let mut is_fullscreen = use_signal(|| false);
    let win = use_window();
    let win2 = win.clone();
    let tray_window = win.window.clone(); // Arc<Window> for tray event callbacks
    // Clone win again because the first effect will move it.
    let win_for_cmd = win.clone();

    use_effect(move || {
        // Explicit read to ensure the effect subscribes to window_cmd changes.
        // `.write().drain(..)` alone may not register a reactive subscription
        // in Dioxus 0.7, causing pushed commands to never be consumed.
        if window_cmd().is_empty() {
            return;
        }
        let cmds: Vec<WindowCmd> = window_cmd.write().drain(..).collect();
        let tao_win: &dioxus::desktop::tao::window::Window = &*win_for_cmd;
        tao_win.set_decorations(false);
        for cmd in cmds {
            match cmd {
                WindowCmd::ToggleFullscreen => {
                    let fs = !is_fullscreen();
                    is_fullscreen.set(fs);
                    if fs {
                        tao_win.set_fullscreen(Some(Fullscreen::Borderless(None)));
                    } else {
                        tao_win.set_fullscreen(None);
                    }
                }
                WindowCmd::ExitFullscreen => {
                    if is_fullscreen() {
                        is_fullscreen.set(false);
                        tao_win.set_fullscreen(None);
                    }
                }
                WindowCmd::Minimize => {
                    tao_win.set_minimized(true);
                }
                WindowCmd::ToggleMaximize => {
                    tao_win.set_maximized(!tao_win.is_maximized());
                }
            }
        }
    });

    // Safety net: returning to Main page always exits fullscreen
    use_effect(move || {
        if matches!(page(), Page::Main) && is_fullscreen() {
            window_cmd.write().push(WindowCmd::ExitFullscreen);
        }
    });

    // ── System tray icon ──────────────────────────────────────────────────
    // Architecture: background OS thread detects events (crossbeam channel is
    // thread-safe), then dispatches via tokio mpsc to the main Dioxus task
    // which executes GTK/window operations on the main thread.
    let mut tray_spawned = use_signal(|| false);
    if !tray_spawned() {
        tray_spawned.set(true);

        init_tray_menu(tray_window);
    }

    // Resize window when switching between Main and Remote views
    use_effect(move || {
        let tao_win: &dioxus::desktop::tao::window::Window = &*win2;
        // Avoid resizing when in fullscreen
        if is_fullscreen() {
            return;
        }
        let (w_res, h_res) = match page() {
            Page::Main => (800_u32, 600_u32),
            Page::Remote => (1280_u32, 800_u32),
        };
        let cur = tao_win.inner_size();
        if cur.width != w_res || cur.height != h_res {
            let _ = tao_win.set_inner_size(desktop::LogicalSize::new(w_res, h_res));
        }
    });

    // ── Auto-start host service on launch ──────────────────────────────────
    {
        let HostCtx {
            state: host_state,
            proc: host_proc,
        } = use_context::<HostCtx>();
        let host_id = local_id();
        let host_pwd = cfg.host_password.clone();
        let stun = load_stun_servers();
        use_future(move || {
            let mut state = host_state;
            let proc_sig = host_proc;
            let host_id = host_id.clone();
            let host_pwd = host_pwd.clone();
            let stun = stun.clone();
            async move {
                if !crate::host_process::is_host_running() {
                    log::info!("[auto-launch] host not running, spawning…");
                    state.set(HostState::Starting);
                    match crate::host_process::HostProcess::spawn(&host_id, &host_pwd, &stun, 15) {
                        Ok(child) => {
                            let shared = proc_sig();
                            let mut guard = shared.lock().unwrap();
                            *guard = Some(child);
                            drop(guard);
                            state.set(HostState::Running);
                            log::info!("[auto-launch] host started successfully");
                        }
                        Err(e) => {
                            log::warn!("[auto-launch] failed: {}", e);
                            state.set(HostState::Error(e));
                        }
                    }
                } else {
                    log::info!("[auto-launch] host already running, skipped");
                }
            }
        });
    }

    rsx! {
        // Bootstrap head — loads Bootstrap 5.3 CSS + Bootstrap Icons
        BootstrapHead {}

        // Bootstrap theme provider — toggles data-bs-theme for Bootstrap components
        ThemeProvider { theme }

        // Main content
        match page() {
            Page::Main   => rsx! { MainWindow { page, server_addr, remote_id, quality, theme, locale, local_id, password } },
            Page::Remote => rsx! { RemoteView { page, server_addr, remote_id, quality, theme, locale, password, window_cmd, is_fullscreen } },
        }
    }
}

fn init_tray_menu(tray_window: Arc<Window>) {
    if let Some(icon) = load_tray_icon() {
        // 1. Create tray icon on the main thread (GTK requires this)
        let _tray = tray::TrayIconBuilder::new()
            .with_tooltip("RTCDesk")
            .with_icon(icon)
            .build()
            .expect("Failed to create tray icon");
        let _ = Box::leak(Box::new(_tray));

        // 2. Channel from background thread → main Dioxus task
        let (tx, mut rx) = tokio::sync::mpsc::channel::<TrayAction>(10);
        let w_arc = tray_window.clone();
        // Main-thread task: receives actions, executes GTK/window ops
        dioxus_core::spawn_forever(async move {
            use tray_menu::{CheckEntry, Divider, PopupMenu, SubMenu, TextEntry};
            while let Some(action) = rx.recv().await {
                match action {
                    TrayAction::LeftClick => {
                        w_arc.set_visible(true);
                        w_arc.set_focus();
                    }
                    TrayAction::RightClick(pos) => {
                        let mut menu = PopupMenu::new();
                        menu.add(&TextEntry::of("open", "Open"));
                        menu.add(&CheckEntry::of("notify", "Notifications", true));
                        menu.add(&Divider);
                        let mut help = SubMenu::of("Help");
                        help.add(&TextEntry::of("about", "About"));
                        menu.add(&help);
                        menu.add(&Divider);
                        menu.add(&TextEntry::of("quit", "Quit"));
                        if let Some(id) = menu.popup(pos) {
                            if id.0 == "quit" {
                                std::process::exit(0);
                            }
                        }
                    }
                }
            }

            log::info!("tray task exited");
        });

        // 3. Background OS thread: polls tray events, sends to main task
        TrayIconEvent::set_event_handler(Some(move |event| {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let _ = tx.send(TrayAction::LeftClick);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } => {
                    match tx.try_send(TrayAction::RightClick(position)) {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!("Failed to send tray action: {:?}", e);
                        }
                    }
                }
                _ => {}
            }
        }));
    } else {
        log::error!("Failed to load tray icon");
    }
}

#[derive(Debug, Clone)]
enum TrayAction {
    LeftClick,
    RightClick(dioxus::desktop::tao::dpi::PhysicalPosition<f64>),
}

// ─── Main window ─────────────────────────────────────────────────────────────

#[component]
fn MainWindow(
    page: Signal<Page>,
    server_addr: Signal<String>,
    remote_id: Signal<String>,
    quality: Signal<usize>,
    theme: Signal<Theme>,
    locale: Signal<String>,
    local_id: Signal<String>,
    password: Signal<String>,
) -> Element {
    let nav_tab = use_signal(|| NavTab::Connect);

    rsx! {
        div {
            style: "display: flex; flex-direction: column; width: 100vw; height: 100vh;
                    background: var(--bs-body-bg); border: 1px solid var(--bs-border-color);
                    border-radius: 10px; overflow: hidden; user-select: none;
                    color: var(--bs-body-color);",

            TitleBar { title: "RTCDesk", theme, locale }

            div {
                class: "d-flex flex-fill overflow-hidden",
                Sidebar { nav_tab, _theme: theme, locale }
                div {
                    class: "flex-fill d-flex flex-column overflow-y-auto",
                    style: "background: var(--bs-body-bg);",
                    match nav_tab() {
                        NavTab::Home     => rsx! { HomeTab    { page, server_addr, remote_id, quality, theme, locale, local_id, password } },
                        NavTab::Connect  => rsx! { ConnectTab { page, server_addr, remote_id, quality, theme, locale, password, local_id } },
                        NavTab::Host     => rsx! { HostTab    { theme, locale, local_id } },
                        NavTab::Settings => rsx! { SettingsTab { server_addr, quality, theme, locale, local_id } },
                    }
                }
            }

            ResizeEdges {}
        }
    }
}
