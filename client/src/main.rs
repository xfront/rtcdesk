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
mod theme;
mod host_process;
mod i18n;
mod quality;
mod remote;
mod viewer;
mod toast;
mod screenshot;

use dioxus::desktop::{self, tao::window::Fullscreen};
use dioxus::prelude::*;
use dioxus::desktop::use_window;
use dioxus_bootstrap_css::prelude::*;
use dioxus::desktop::trayicon::DioxusTrayIcon;
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
use crate::config::{load_config, load_stun_servers, save_config};
use crate::remote::view::RemoteView;
use crate::config::ThemeConfig;
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

    let cfg = desktop::Config::new()
        .with_window(win_builder)
        .with_close_behaviour(WindowCloseBehaviour::WindowHides)
        .with_tray_icon_show_window_on_click(true);

    dioxus::LaunchBuilder::new()
        .with_cfg(cfg)
        .launch(app);
}

fn load_tray_icon() -> Option<DioxusTrayIcon> {
    let bytes = include_bytes!("assets/favicon.webp");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    DioxusTrayIcon::from_rgba(rgba.into_raw(), w, h).ok()
}
fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let bytes = include_bytes!("assets/favicon.webp");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

// ─── Root component ───────────────────────────────────────────────────────────

#[component]
fn app() -> Element {
    // Inject custom app.css (minimal — only window chrome, remote session, theme vars)
    let mut css_injected = use_signal(|| false);
    use_effect(move || {
        if css_injected() { return; }
        css_injected.set(true);
        let css = include_str!("assets/app.css");
        let escaped = css.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
        let _ = document::eval(&format!(
            "var s = document.createElement('style'); s.textContent = `{}`; document.head.appendChild(s);",
            escaped
        ));
    });

    let cfg         = load_config();
    let page        = use_signal(|| Page::Main);
    let server_addr = use_signal(move || cfg.server_addr.clone());
    let remote_id   = use_signal(move || cfg.remote_id.clone());
    let quality     = use_signal(move || cfg.quality);
    let theme       = use_signal(move || cfg.theme.0);
    let locale      = use_signal(move || cfg.locale.clone());
    let local_id    = use_signal(move || cfg.local_id.clone());
    let password    = use_signal(String::new);

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
    let win_tray = win.clone(); // captured for tray menu "Show" handler

    use_effect(move || {
        // Explicit read to ensure the effect subscribes to window_cmd changes.
        // `.write().drain(..)` alone may not register a reactive subscription
        // in Dioxus 0.7, causing pushed commands to never be consumed.
        if window_cmd().is_empty() { return; }
        let cmds: Vec<WindowCmd> = window_cmd.write().drain(..).collect();
        let tao_win: &dioxus::desktop::tao::window::Window = &*win;
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
                WindowCmd::Minimize => { tao_win.set_minimized(true); }
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
    let mut tray_initialized = use_signal(|| false);
    use_effect(move || {
        if tray_initialized() { return; }
        tray_initialized.set(true);
        let icon = load_tray_icon();

        // Build a custom menu with MenuItem instead of PredefinedMenuItem
        // because PredefinedMenuItem::quit is unsupported on Linux.
        use dioxus::desktop::trayicon::menu::{Menu, MenuItem};
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id("show", "&Show RTCDesk", true, None),
            &MenuItem::with_id("quit", "&Quit", true, None),
        ]).ok();

        dioxus::desktop::trayicon::init_tray_icon(menu, icon);
    });

    // Handle tray menu events (Show / Quit)
    // NOTE: tray_icon::menu::MenuEvent is a re-export of muda::MenuEvent,
    // and dioxus-desktop dispatches these as UserWindowEvent::MudaMenuEvent.
    dioxus::desktop::use_muda_event_handler(move |event| {
        match event.id.as_ref() {
            "show" => {
                let w: &dioxus::desktop::tao::window::Window = &*win_tray;
                w.set_visible(true);
                w.set_focus();
            }
            "quit" => {
                std::process::exit(0);
            }
            _ => {}
        }
    });

    // Resize window when switching between Main and Remote views
    use_effect(move || {
        let tao_win: &dioxus::desktop::tao::window::Window = &*win2;
        // Avoid resizing when in fullscreen
        if is_fullscreen() { return; }
        let (w_res, h_res) = match page() {
            Page::Main   => (800_u32, 600_u32),
            Page::Remote => (1280_u32, 800_u32),
        };
        let cur = tao_win.inner_size();
        if cur.width != w_res || cur.height != h_res {
            let _ = tao_win.set_inner_size(desktop::LogicalSize::new(w_res, h_res));
        }
    });

    // ── Auto-start host service on launch ──────────────────────────────────
    {
        let HostCtx { state: host_state, proc: host_proc } = use_context::<HostCtx>();
        let host_id  = local_id();
        let host_pwd = cfg.host_password.clone();
        let stun     = load_stun_servers();
        use_future(move || {
            let mut state = host_state;
            let proc_sig  = host_proc;
            let host_id   = host_id.clone();
            let host_pwd  = host_pwd.clone();
            let stun      = stun.clone();
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

// ─── Main window ─────────────────────────────────────────────────────────────

#[component]
fn MainWindow(
    page:        Signal<Page>,
    server_addr: Signal<String>,
    remote_id:   Signal<String>,
    quality:     Signal<usize>,
    theme:       Signal<Theme>,
    locale:      Signal<String>,
    local_id:    Signal<String>,
    password:    Signal<String>,
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
