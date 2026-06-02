//! Host tab: start/stop the desktop sharing service, show ID and password.
//! Also provides systemd user service install/uninstall on Linux.
//! Refactored with dioxus-bootstrap-css components.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use dioxus_bootstrap_css::theme::Theme;
use crate::clipboard::set_system_clipboard;
use crate::host_process::{HostProcess, SharedHostProcess, is_systemd_service_installed,
                          is_systemd_service_running, install_systemd_service,
                          uninstall_systemd_service, update_systemd_env,
                          restart_systemd_service};

// ─── Host service state ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum HostState {
    Stopped,
    Starting,
    Running,
    Error(String),
}

#[derive(Clone, Copy)]
pub struct HostCtx {
    pub state: Signal<HostState>,
    pub proc: Signal<SharedHostProcess>,
}

impl HostCtx {
    pub fn new() -> Self {
        Self {
            state: Signal::new(HostState::Stopped),
            proc: Signal::new(std::sync::Arc::new(std::sync::Mutex::new(None))),
        }
    }
}

/// Status message for systemd service install/uninstall operations.
#[derive(Debug, Clone, PartialEq)]
pub enum SvcMsg {
    Installing,
    Uninstalling,
    Success(String),
    Error(String),
}

// ─── Host tab ────────────────────────────────────────────────────────────────

#[component]
pub fn HostTab(
    theme: Signal<Theme>,
    locale: Signal<String>,
    local_id: Signal<String>,
) -> Element {
    let _t = theme();
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let HostCtx { state: mut host_state, proc: host_proc } = use_context::<HostCtx>();
    let mut host_password = use_signal(move || load_config().host_password.clone());
    let my_id = local_id();
    let id_for_install  = my_id.clone();
    let id_for_restart  = my_id.clone();
    let id_for_spawn    = my_id.clone();
    let id_for_pw_blur  = my_id.clone();
    let id_for_pw_rand  = my_id.clone();
    let id_for_pw_clear = my_id.clone();

    let svc_installed = use_signal(is_systemd_service_installed);
    let svc_msg: Signal<Option<SvcMsg>> = use_signal(|| None);

    use_future(move || {
        let proc = host_proc();
        let mut hs = host_state.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let mut guard = proc.lock().unwrap();
                if let Some(ref mut p) = *guard {
                    if !p.is_running() {
                        *guard = None;
                        drop(guard);
                        hs.set(HostState::Error(tr(keys::HOST_PROCESS_EXITED, l).to_string()));
                    }
                }
            }
        }
    });

    let proc = host_proc();
    let (status_color, status_text, status_icon) = match host_state() {
        HostState::Stopped  => ("var(--bs-tertiary-color)", tr(keys::HOST_STOPPED, l),  "⏹"),
        HostState::Starting => ("var(--bs-warning)",     tr(keys::HOST_STARTING, l), "⏳"),
        HostState::Running  => ("var(--bs-success)",   tr(keys::HOST_RUNNING, l),  "▶"),
        HostState::Error(_) => ("var(--bs-danger)",   tr(keys::HOST_ERROR, l),   "⚠"),
    };

    let is_running = matches!(host_state(), HostState::Running);
    let btn_label = if is_running { tr(keys::HOST_BTN_STOP, l) } else { tr(keys::HOST_BTN_START, l) };
    let btn_bg = if is_running { "var(--bs-danger)" } else { "var(--bs-success)" };

    let svc_installed_val = svc_installed();
    let (svc_status_icon, svc_status_label, svc_status_color) = if svc_installed_val {
        let running = is_systemd_service_running();
        if running {
            ("◉", tr(keys::HOST_SERVICE_INSTALLED, l), "var(--bs-success)")
        } else {
            ("○", tr(keys::HOST_SERVICE_INSTALLED, l), "var(--bs-warning)")
        }
    } else {
        ("✗", tr(keys::HOST_SERVICE_NOT_INSTALLED, l), "var(--bs-tertiary-color)")
    };
    let svc_hint = if svc_installed_val {
        if cfg!(target_os = "linux") { tr(keys::HOST_SERVICE_INSTALLED_HINT, l) }
        else if cfg!(target_os = "windows") { tr(keys::HOST_SERVICE_INSTALLED_HINT_WIN, l) }
        else { tr(keys::HOST_SERVICE_INSTALLED_HINT, l) }
    } else {
        if cfg!(target_os = "linux") { tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT, l) }
        else if cfg!(target_os = "windows") { tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT_WIN, l) }
        else { tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT, l) }
    };

    let svc_status_line = svc_msg().map(|msg| match msg {
        SvcMsg::Installing   => ("⏳", "var(--bs-warning)",   tr(keys::HOST_SVC_INSTALLING, l).to_string()),
        SvcMsg::Uninstalling => ("⏳", "var(--bs-warning)",   tr(keys::HOST_SVC_UNINSTALLING, l).to_string()),
        SvcMsg::Success(ref s) => ("✓", "var(--bs-success)", s.clone()),
        SvcMsg::Error(ref s)   => ("✗", "var(--bs-danger)", s.clone()),
    });

    rsx! {
        div { class: "p-4",

            h4 { class: "mb-2", "{tr(keys::HOST_TITLE, l)}" }
            p  { class: "text-secondary small mb-4", "{tr(keys::HOST_SUBTITLE, l)}" }

            // ── System service status card ──
            Card { class: "mb-3",
                body: rsx! {
                    div { class: "d-flex align-items-center justify-content-between mb-2",
                        div { class: "d-flex align-items-center gap-2",
                            span { style: "font-size: 15px;", "{svc_status_icon}" }
                            span {
                                style: "font-size: 13px; font-weight: 600; color: {svc_status_color};",
                                "{svc_status_label}"
                            }
                        }
                        if svc_installed_val {
                            Button {
                                color: Color::Danger,
                                outline: true,
                                size: Size::Sm,
                                onclick: {
                                    let installed = svc_installed;
                                    let mut msg = svc_msg;
                                    let hs = host_state;
                                    let proc_sig = host_proc;
                                    move |_| {
                                        msg.set(Some(SvcMsg::Uninstalling));
                                        let mut msg_clone = msg;
                                        let mut installed_clone = installed;
                                        let mut hs_clone = hs;
                                        let proc_sig = proc_sig;
                                        spawn(async move {
                                            match uninstall_systemd_service() {
                                                Ok(()) => {
                                                    installed_clone.set(false);
                                                    msg_clone.set(Some(SvcMsg::Success(
                                                        tr(keys::HOST_SVC_UNINSTALL_OK, l).to_string()
                                                    )));
                                                }
                                                Err(e) => {
                                                    msg_clone.set(Some(SvcMsg::Error(
                                                        tf(keys::HOST_SVC_UNINSTALL_ERR, l, &[("e", &e)])
                                                    )));
                                                }
                                            }
                                            let shared = proc_sig();
                                            let mut guard = shared.lock().unwrap();
                                            if let Some(ref mut p) = *guard { p.stop(); }
                                            *guard = None;
                                            drop(guard);
                                            hs_clone.set(HostState::Stopped);
                                        });
                                    }
                                },
                                "{tr(keys::HOST_BTN_UNINSTALL_SVC, l)}"
                            }
                        } else {
                            Button {
                                color: Color::Primary,
                                size: Size::Sm,
                                onclick: {
                                    let installed = svc_installed;
                                    let mut msg = svc_msg;
                                    let hs = host_state;
                                    let proc_sig = host_proc;
                                    let id = id_for_install.clone();
                                    let pwd = host_password();
                                    let stun = crate::config::load_stun_servers();
                                    move |_| {
                                        msg.set(Some(SvcMsg::Installing));
                                        let mut msg_clone = msg;
                                        let mut installed_clone = installed;
                                        let mut hs_clone = hs;
                                        let proc_sig = proc_sig;
                                        let id = id.clone();
                                        let pwd = pwd.clone();
                                        let stun = stun.clone();
                                        spawn(async move {
                                            match install_systemd_service(&id, &pwd, &stun, 15) {
                                                Ok(()) => {
                                                    installed_clone.set(true);
                                                    msg_clone.set(Some(SvcMsg::Success(
                                                        tr(keys::HOST_SVC_INSTALL_OK, l).to_string()
                                                    )));
                                                    let shared = proc_sig();
                                                    let mut guard = shared.lock().unwrap();
                                                    if let Some(ref mut p) = *guard { p.stop(); }
                                                    *guard = None;
                                                    drop(guard);
                                                    hs_clone.set(HostState::Stopped);
                                                }
                                                Err(e) => {
                                                    msg_clone.set(Some(SvcMsg::Error(
                                                        tf(keys::HOST_SVC_INSTALL_ERR, l, &[("e", &e)])
                                                    )));
                                                }
                                            }
                                        });
                                    }
                                },
                                "{tr(keys::HOST_BTN_INSTALL_SVC, l)}"
                            }
                        }
                    }
                    p {
                        class: "text-secondary small mb-0 lh-sm",
                        "{svc_hint}"
                    }
                    if let Some((icon, color, text)) = svc_status_line {
                        p {
                            class: "small mt-2 mb-0",
                            style: "color: {color};",
                            "{icon} {text}"
                        }
                    }
                }
            }

            // ── Status & start/stop card ──
            Card { class: "mb-3",
                body: rsx! {
                    div { class: "d-flex align-items-center justify-content-between mb-3",
                        div { class: "d-flex align-items-center gap-2",
                            span { style: "font-size: 20px;", "{status_icon}" }
                            span {
                                style: "font-size: 15px; font-weight: 600; color: {status_color};",
                                "{status_text}"
                            }
                        }
                        Button {
                            color: if is_running { Color::Danger } else { Color::Success },
                            style: "background: {btn_bg}; border-color: {btn_bg};",
                            onclick: move |_| {
                                if is_running {
                                    let proc = host_proc();
                                    let mut hs = host_state.clone();
                                    spawn(async move {
                                        let mut guard = proc.lock().unwrap();
                                        if let Some(ref mut p) = *guard { p.stop(); }
                                        *guard = None;
                                        drop(guard);
                                        hs.set(HostState::Stopped);
                                    });
                                } else {
                                    if svc_installed() && is_systemd_service_running() {
                                        let mut hs = host_state.clone();
                                        let mut si = svc_installed;
                                        let restart_id = id_for_restart.clone();
                                        spawn(async move {
                                            let pwd = host_password();
                                            let stun = crate::config::load_stun_servers();
                                            update_systemd_env(&restart_id, &pwd, &stun, 15);
                                            match restart_systemd_service() {
                                                Ok(()) => {}
                                                Err(e) => {
                                                    log::warn!("Failed to restart systemd service: {}", e);
                                                    si.set(false);
                                                    hs.set(HostState::Error(e));
                                                }
                                            }
                                        });
                                    } else {
                                        host_state.set(HostState::Starting);
                                        let id = id_for_spawn.clone();
                                        let pwd = host_password();
                                        let stun = crate::config::load_stun_servers();
                                        let proc = host_proc();
                                        let mut hs = host_state.clone();
                                        spawn(async move {
                                            match HostProcess::spawn(&id, &pwd, &stun, 15) {
                                                Ok(child) => {
                                                    let mut guard = proc.lock().unwrap();
                                                    *guard = Some(child);
                                                    drop(guard);
                                                    hs.set(HostState::Running);
                                                }
                                                Err(e) => { hs.set(HostState::Error(e)); }
                                            }
                                        });
                                    }
                                }
                            },
                            "{btn_label}"
                        }
                    }

                    // Device ID row
                    div {
                        class: "d-flex align-items-center justify-content-between
                                p-3 rounded border mb-2",
                        style: "background: var(--bs-tertiary-bg); border-color: var(--bs-border-color);",
                        span { class: "text-secondary small text-uppercase", "{tr(keys::HOST_LABEL_ID, l)}" }
                        div { class: "d-flex align-items-center gap-2",
                            span {
                                style: "font-size: 18px; font-weight: 700; color: var(--bs-success);
                                        letter-spacing: 2px;",
                                if my_id.is_empty() { "—" } else { "{format_local_id(&my_id)}" }
                            }
                            Button {
                                color: Color::Primary,
                                outline: true,
                                size: Size::Sm,
                                onclick: {
                                    let id = local_id();
                                    move |_| { let _ = set_system_clipboard(&id); }
                                },
                                "{tr(keys::HOST_BTN_COPY, l)}"
                            }
                        }
                    }

                    // Password row
                    {
                        let pw_display = if host_password().is_empty() {
                            tr(keys::HOST_PW_OPEN, l).to_string()
                        } else {
                            host_password()
                        };
                        rsx! {
                            div {
                                class: "d-flex align-items-center justify-content-between
                                        p-3 rounded border",
                                style: "background: var(--bs-tertiary-bg); border-color: var(--bs-border-color);",
                                span { class: "text-secondary small text-uppercase", "{tr(keys::HOST_LABEL_PW, l)}" }
                                div { class: "d-flex align-items-center gap-2",
                                    span {
                                        style: "font-size: 16px; font-weight: 600; color: var(--bs-body-color);
                                                letter-spacing: 3px;",
                                        "{pw_display}"
                                    }
                                    if !host_password().is_empty() {
                                        Button {
                                            color: Color::Primary,
                                            outline: true,
                                            size: Size::Sm,
                                            onclick: {
                                                let pwd = host_password();
                                                move |_| { let _ = set_system_clipboard(&pwd); }
                                            },
                                            "{tr(keys::HOST_BTN_COPY, l)}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Password settings ──
            Card { class: "mb-3",
                body: rsx! {
                    h6 { class: "mb-3", "{tr(keys::HOST_SECTION_PW, l)}" }
                    div { class: "d-flex gap-2 align-items-end",
                        div { class: "flex-grow-1",
                            label { class: "form-label small", "{tr(keys::HOST_FIELD_PW, l)}" }
                            input {
                                class: "form-control",
                                r#type: "password",
                                placeholder: tr(keys::HOST_PLACEHOLDER_PW, l),
                                value: "{host_password}",
                                oninput: move |e| host_password.set(e.value()),
                                onblur: {
                                    let si = svc_installed;
                                    let id = id_for_pw_blur.clone();
                                    let stun = crate::config::load_stun_servers();
                                    move |_| {
                                        let mut cfg = load_config();
                                        cfg.host_password = host_password();
                                        save_config(&cfg);
                                        if si() { update_systemd_env(&id, &host_password(), &stun, 15); }
                                    }
                                },
                            }
                        }
                        Button {
                            color: Color::Secondary,
                            outline: true,
                            title: tr(keys::HOST_BTN_RANDOM_TOOLTIP, l),
                            onclick: {
                                let si = svc_installed;
                                let id = id_for_pw_rand.clone();
                                let stun = crate::config::load_stun_servers();
                                move |_| {
                                    let pwd = crate::local_id::rand_id()[..6].to_string();
                                    host_password.set(pwd.clone());
                                    let mut cfg = load_config();
                                    cfg.host_password = pwd;
                                    save_config(&cfg);
                                    if si() { update_systemd_env(&id, &host_password(), &stun, 15); }
                                }
                            },
                            "{tr(keys::HOST_BTN_RANDOM, l)}"
                        }
                        Button {
                            color: Color::Danger,
                            outline: true,
                            onclick: {
                                let si = svc_installed;
                                let id = id_for_pw_clear.clone();
                                let stun = crate::config::load_stun_servers();
                                move |_| {
                                    host_password.set(String::new());
                                    let mut cfg = load_config();
                                    cfg.host_password = String::new();
                                    save_config(&cfg);
                                    if si() { update_systemd_env(&id, "", &stun, 15); }
                                }
                            },
                            "{tr(keys::HOST_BTN_CLEAR, l)}"
                        }
                    }
                    p {
                        class: "text-secondary small mt-2 mb-0 lh-sm",
                        "{tr(keys::HOST_PW_HELP, l)}"
                    }
                }
            }

            if is_running {
                {
                    let pid_text = {
                        let guard = proc.lock().unwrap();
                        guard.as_ref().map(|p| format!("PID: {}", p.pid().unwrap_or(0))).unwrap_or_default()
                    };
                    let local_ip = local_ip_address::local_ip()
                        .map(|ip| format!("{}:9009", ip))
                        .unwrap_or_else(|_| "0.0.0.0:9009".to_string());
                    let sig_addr = tf(keys::HOST_SIGNAL_ADDR, l, &[("ip", &local_ip)]);
                    rsx! {
                        Alert {
                            color: Color::Success,
                            h6 { class: "alert-heading", "{tr(keys::HOST_RUNNING_TITLE, l)}" }
                            p { class: "mb-0 small lh-base",
                                "{tr(keys::HOST_RUNNING_HINT, l)}"
                                br {}
                                "{sig_addr}"
                                if !pid_text.is_empty() {
                                    br {}
                                    "{pid_text}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
