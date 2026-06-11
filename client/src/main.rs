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
mod connect;
mod decoder;
mod frame_slot;
mod host_process;
mod i18n;
mod quality;
mod remote;
mod rpc_client;
mod screenshot;
mod theme;
mod toast;
mod viewer;
mod client_callback;

use crate::components::icon::{load_tray_icon, load_window_icon};
use crate::frame_slot::{FrameSlot, SharedFrameSlot, FRAME_PROTOCOL_NAME};
use dioxus::desktop::use_window;
use dioxus::desktop::{self, tao::window::Fullscreen};
use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
use std::sync::{Arc, Mutex};
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
use crate::components::settings_dialog::SettingsDialog;
use crate::components::sidebar::{NavTab, Sidebar};
use crate::components::titlebar::TitleBar;
use crate::config::ThemeConfig;
use crate::config::{load_config, load_stun_servers, save_config};
use crate::remote::view::RemoteView;
use dioxus_bootstrap_css::theme::Theme;
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
    // rustls crypto provider no longer needed (ice0 uses TCP directly)
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

    // Provide shared RPC client context
    let rpc_client = use_context_provider(|| std::sync::Arc::new(std::sync::Mutex::new(crate::rpc_client::RpcClient::new())));

    // Provide shared Host context (persists across tab switches)
    let host_ctx = use_context_provider(|| HostCtx::new());

    // Provide shared Auth context (persists across tab switches)
    let auth_ctx = AuthCtx::new(rpc_client.clone());
    use_context_provider(|| auth_ctx.clone());
    
    // ── Auto-start host service on launch (MUST happen BEFORE RPC and auto-login) ──
    let host_started_signal = use_signal(|| false);
    {
        let HostCtx {
            state: host_state,
            proc: host_proc,
            client_callback: _,
        } = use_context::<HostCtx>();
        let host_id = local_id();
        use_future(move || {
            let mut state = host_state;
            let proc_sig = host_proc;
            let host_id = host_id.clone();
            let mut started = host_started_signal;
            async move {
                if !crate::host_process::is_host_running() {
                    log::info!("[auto-launch] host not running, spawning…");
                    state.set(HostState::Starting);
                    match crate::host_process::HostProcess::spawn(&host_id, 15) {
                        Ok(child) => {
                            let shared = proc_sig();
                            let mut guard = shared.lock().unwrap();
                            *guard = Some(child);
                            drop(guard);
                            state.set(HostState::Running);
                            started.set(true);
                            log::info!("[auto-launch] host started successfully");
                        }
                        Err(e) => {
                            log::warn!("[auto-launch] failed: {}", e);
                            state.set(HostState::Error(e));
                        }
                    }
                } else {
                    log::info!("[auto-launch] host already running, skipped");
                    started.set(true);
                }
            }
        });
    }
    
    // Initialize RPC client connection to host-service
    {
        let rpc_client_for_init = rpc_client.clone();
        let host_started = host_started_signal.clone();
        use_future(move || {
            let rpc_client = rpc_client_for_init.clone();
            async move {
                // Wait for host to start
                let mut retries = 0;
                while !host_started() && retries < 20 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    retries += 1;
                }
                
                if !host_started() {
                    log::warn!("[RPC] Host did not start in time, will retry connection later");
                }
                
                // Try to connect to RPC with retries
                for attempt in 1..=5 {
                    let connect_result = {
                        if let Ok(mut client) = rpc_client.lock() {
                            client.connect().await
                        } else {
                            break;
                        }
                    };
                    
                    match connect_result {
                        Ok(_) => {
                            log::info!("[RPC] Connected to host-service");
                            return; // Success
                        }
                        Err(e) => {
                            log::warn!("[RPC] Connection attempt {} failed: {}", attempt, e);
                            if attempt < 5 {
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                }
                log::warn!("[RPC] Failed to connect to host-service after 5 attempts");
            }
        });
    }

    // Auto-login on startup: delayed until RPC is ready
    let auto_login_locale = locale();
    use_effect(move || {
        // Only attempt auto-login after a delay to give IPC time to connect
        let auth_ctx_clone = auth_ctx.clone();
        let locale_clone = auto_login_locale.clone();
        spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            auth_ctx_clone.try_auto_login(locale_clone);
        });
    });

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
    // Use the `tray` crate (NOT Dioxus built-in tray-icon) for tray events.
    //
    // A dedicated std::thread polls `TrayIconEvent::receiver()` and sends
    // tray actions through a crossbeam channel. A `use_future` on the Dioxus
    // side receives the actions and executes all UI operations on the Dioxus
    // event loop (which IS the GTK main thread) — no glib needed.
    let (tray_tx, tray_rx) = crossbeam_channel::unbounded::<TrayAction>();
    let mut tray_spawned = use_signal(|| false);
    if !tray_spawned() {
        tray_spawned.set(true);
        init_tray_icon(tray_window, tray_tx);
    }

    // Handle tray actions on the Dioxus event loop (= GTK main thread)
    {
        let window_for_tray = win.window.clone();
        use_future(move || {
            let rx = tray_rx.clone();
            let w = window_for_tray.clone();
            async move {
                loop {
                    // Use spawn_blocking to wait on the crossbeam receiver
                    // without blocking the async runtime.
                    let rx = rx.clone();
                    let action = tokio::task::spawn_blocking(move || {
                        rx.recv_timeout(std::time::Duration::from_millis(200))
                    })
                    .await;

                    let Ok(Ok(action)) = action else {
                        continue;
                    };

                    match action {
                        TrayAction::LeftClick => {
                            log::info!("[tray] left click — showing window");
                            w.set_visible(true);
                            w.set_focus();
                        }
                        TrayAction::RightClick(position) => {
                            log::info!("[tray] right click — showing popup menu");
                            use tray_menu::{Divider, PopupMenu, TextEntry};
                            let mut menu = PopupMenu::new();
                            menu.add(&TextEntry::of("open", "Open"));
                            menu.add(&Divider);
                            menu.add(&TextEntry::of("quit", "Quit"));
                            if let Some(id) = menu.popup(position) {
                                match id.0.as_str() {
                                    "open" => {
                                        w.set_visible(true);
                                        w.set_focus();
                                    }
                                    "quit" => {
                                        std::process::exit(0);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        });
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

/// Initialize the system tray icon using the `tray` crate.
///
/// We deliberately **avoid** Dioxus's built-in `tray-icon` module and instead
/// use the standalone `tray` crate with its original event channel
/// (`TrayIconEvent::receiver()`). Events are polled from a dedicated
/// `std::thread`, which is completely independent of the Dioxus async runtime
/// and continues working even when the window is hidden.
///
/// The thread sends tray actions through a crossbeam channel to a
/// `use_future` on the Dioxus side, which runs on the Dioxus event loop
/// (= GTK main thread). All UI operations (popup menu, window toggle) happen
/// there — no glib needed.
fn init_tray_icon(window: Arc<Window>, tx: crossbeam_channel::Sender<TrayAction>) {
    let icon = match load_tray_icon() {
        Some(i) => i,
        None => {
            log::error!("Failed to load tray icon");
            return;
        }
    };

    // 1. Create tray icon on the main thread (GTK requires this)
    let _tray = tray::TrayIconBuilder::new()
        .with_tooltip("RTCDesk")
        .with_icon(icon)
        .build()
        .expect("Failed to create tray icon");
    // Leak to keep alive for the process lifetime
    let _ = Box::leak(Box::new(_tray));

    // 2. Background OS thread: polls tray events, sends to main task
    tray::TrayIconEvent::set_event_handler(Some(move |event| {
        match event {
            tray::TrayIconEvent::Click {
                button: tray::MouseButton::Left,
                button_state: tray::MouseButtonState::Up,
                ..
            } => {
                window.set_visible(true);
                window.set_focus();
                let _ = tx.send(TrayAction::LeftClick);
            }
            tray::TrayIconEvent::Click {
                button: tray::MouseButton::Right,
                button_state: tray::MouseButtonState::Up,
                position,
                ..
            } => {
                log::info!("[tray] right click detected — sending to Dioxus event loop");
                // Popup menu MUST run on the GTK main thread.
                // Send the position to the Dioxus event loop via channel.
                let _ = tx.send(TrayAction::RightClick(position));
            }
            _ => {}
        }
    }));
}

// ─── Tray action types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum TrayAction {
    LeftClick,
    RightClick(tray::dpi::PhysicalPosition<f64>),
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
    let show_settings_dialog = use_signal(|| false);
    let show_lock_dialog = use_signal(|| false);

    rsx! {
        div {
            style: "display: flex; flex-direction: column; width: 100vw; height: 100vh;
                    background: var(--bs-body-bg); border: 1px solid var(--bs-border-color);
                    border-radius: 10px; overflow: hidden; user-select: none;
                    color: var(--bs-body-color);",

            TitleBar { title: "RTCDesk", theme, locale, show_settings_dialog, show_lock_dialog }

            div {
                class: "d-flex flex-fill overflow-hidden",
                Sidebar { nav_tab, _theme: theme, locale, show_lock_dialog }
                div {
                    class: "flex-fill d-flex flex-column overflow-y-auto",
                    style: "background: var(--bs-body-bg);",
                    match nav_tab() {
                        NavTab::Home     => rsx! { HomeTab    { page, server_addr, remote_id, quality, theme, locale, local_id, password } },
                        NavTab::Connect  => rsx! { ConnectTab { page, server_addr, remote_id, quality, theme, locale, password, local_id } },
                        NavTab::Host     => rsx! { HostTab    { theme, locale, local_id } },
                    }
                }
            }

            ResizeEdges {}
        }

        // Settings dialog overlay
        SettingsDialog { show: show_settings_dialog, theme, locale, server_addr, quality, local_id }
    }
}
