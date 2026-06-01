//! Host tab: start/stop the desktop sharing service, show ID and password.
//! Also provides systemd user service install/uninstall on Linux.

use dioxus::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use crate::theme::{tc, Theme};
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
    let t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let HostCtx { state: mut host_state, proc: host_proc } = use_context::<HostCtx>();
    let mut host_password = use_signal(move || load_config().host_password.clone());
    let my_id = local_id();
    // Clone for use in event closures (Dioxus rsx! creates a single outer move closure,
    // so we need independent clones for each nested capture site).
    let id_for_install  = my_id.clone();
    let id_for_restart  = my_id.clone();
    let id_for_spawn    = my_id.clone();
    let id_for_pw_blur  = my_id.clone();
    let id_for_pw_rand  = my_id.clone();
    let id_for_pw_clear = my_id.clone();

    // systemd service status — check once on mount
    let mut svc_installed = use_signal(is_systemd_service_installed);
    let mut svc_msg: Signal<Option<SvcMsg>> = use_signal(|| None);

    // Periodically check if the child process is still alive
    use_future(move || {
        let mut proc = host_proc();
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
        HostState::Stopped => (t.text_dim, tr(keys::HOST_STOPPED, l), "⏹"),
        HostState::Starting => (t.warn, tr(keys::HOST_STARTING, l), "⏳"),
        HostState::Running => (t.accent, tr(keys::HOST_RUNNING, l), "▶"),
        HostState::Error(_) => (t.danger, tr(keys::HOST_ERROR, l), "⚠"),
    };

    let is_running = matches!(host_state(), HostState::Running);
    let btn_label = if is_running { tr(keys::HOST_BTN_STOP, l) } else { tr(keys::HOST_BTN_START, l) };
    let btn_bg = if is_running { t.danger } else { t.accent };

    let copy_label = tr(keys::HOST_BTN_COPY, l);
    let pw_open = tr(keys::HOST_PW_OPEN, l);

    // systemd service status display
    let svc_installed_val = svc_installed();
    let (svc_status_icon, svc_status_label, svc_status_color) = if svc_installed_val {
        let running = is_systemd_service_running();
        if running {
            ("◉", tr(keys::HOST_SERVICE_INSTALLED, l), t.accent)
        } else {
            ("○", tr(keys::HOST_SERVICE_INSTALLED, l), t.warn)
        }
    } else {
        ("✗", tr(keys::HOST_SERVICE_NOT_INSTALLED, l), t.text_dim)
    };
    let svc_not_installed_hint = if cfg!(target_os = "linux") {
        tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT, l)
    } else if cfg!(target_os = "windows") {
        tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT_WIN, l)
    } else {
        tr(keys::HOST_SERVICE_NOT_INSTALLED_HINT, l)
    };
    let svc_installed_hint = if cfg!(target_os = "linux") {
        tr(keys::HOST_SERVICE_INSTALLED_HINT, l)
    } else if cfg!(target_os = "windows") {
        tr(keys::HOST_SERVICE_INSTALLED_HINT_WIN, l)
    } else {
        tr(keys::HOST_SERVICE_INSTALLED_HINT, l)
    };
    let svc_hint = if svc_installed_val { svc_installed_hint } else { svc_not_installed_hint };

    // svc action messages
    let svc_status_line = svc_msg().map(|msg| match msg {
        SvcMsg::Installing => ("⏳", t.warn, tr(keys::HOST_SVC_INSTALLING, l).to_string()),
        SvcMsg::Uninstalling => ("⏳", t.warn, tr(keys::HOST_SVC_UNINSTALLING, l).to_string()),
        SvcMsg::Success(ref s) => ("✓", t.accent, s.clone()),
        SvcMsg::Error(ref s) => ("✗", t.danger, s.clone()),
    });

    rsx! {
        div {
            class: "page",

            h2 { class: "heading-lg", "{tr(keys::HOST_TITLE, l)}" }
            p { class: "subtitle", "{tr(keys::HOST_SUBTITLE, l)}" }

            // ── System service status card ──
            div {
                class: "card mb-16",
                div {
                    class: "flex-between mb-12",
                    div {
                        class: "flex-center-row",
                        style: "gap: 8px;",
                        span { style: "font-size: 15px;", "{svc_status_icon}" }
                        span {
                            style: "font-size: 13px; font-weight: 600; color: {svc_status_color};",
                            "{svc_status_label}"
                        }
                    }
                    if svc_installed_val {
                        button {
                            class: "btn-outline-danger",
                            style: "font-size: 11px;",
                            onclick: {
                                let mut installed = svc_installed;
                                let mut msg = svc_msg;
                                let mut hs = host_state;
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
                                        // After uninstall, stop any running child process
                                        let shared = proc_sig();
                                        let mut guard = shared.lock().unwrap();
                                        if let Some(ref mut p) = *guard {
                                            p.stop();
                                        }
                                        *guard = None;
                                        drop(guard);
                                        hs_clone.set(HostState::Stopped);
                                    });
                                }
                            },
                            "{tr(keys::HOST_BTN_UNINSTALL_SVC, l)}"
                        }
                    } else {
                        button {
                            class: "btn-primary-sm",
                            style: "font-size: 11px;",
                            onclick: {
                                let mut installed = svc_installed;
                                let mut msg = svc_msg;
                                let mut hs = host_state;
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
                                                // Stop any running child process — systemd handles it now
                                                let shared = proc_sig();
                                                let mut guard = shared.lock().unwrap();
                                                if let Some(ref mut p) = *guard {
                                                    p.stop();
                                                }
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
                    class: "text-dim text-11",
                    style: "line-height: 1.5; margin: 0;",
                    "{svc_hint}"
                }
                if let Some((icon, color, text)) = svc_status_line {
                    p {
                        class: "text-12",
                        style: "margin: 10px 0 0; color: {color};",
                        "{icon} {text}"
                    }
                }
            }

            // ── Status & start/stop card ──
            div {
                class: "card mb-16",

                div {
                    class: "flex-between mb-16",
                    div {
                        class: "flex-center-row",
                        style: "gap: 10px;",
                        span { style: "font-size: 20px;", "{status_icon}" }
                        span {
                            style: "font-size: 15px; font-weight: 600; color: {status_color};",
                            "{status_text}"
                        }
                    }
                    button {
                        class: "btn-primary-sm",
                        style: "background: {btn_bg};",
                        onclick: move |_| {
                            if is_running {
                                let proc = host_proc();
                                let mut hs = host_state.clone();
                                spawn(async move {
                                    let mut guard = proc.lock().unwrap();
                                    if let Some(ref mut p) = *guard {
                                        p.stop();
                                    }
                                    *guard = None;
                                    drop(guard);
                                    hs.set(HostState::Stopped);
                                });
                            } else {
                                // If systemd service installed and running, restart instead of spawning child
                                if svc_installed() && is_systemd_service_running() {
                                    let mut hs = host_state.clone();
                                    let mut si = svc_installed;
                                    let restart_id = id_for_restart.clone();
                                    spawn(async move {
                                        // Update env file and restart
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
                                            Err(e) => {
                                                hs.set(HostState::Error(e));
                                            }
                                        }
                                    });
                                }
                            }
                        },
                        "{btn_label}"
                    }
                }

                div {
                    class: "value-row mb-12",
                    div { class: "label-sm", "{tr(keys::HOST_LABEL_ID, l)}" }
                    div {
                        class: "flex-center-row",
                        span {
                            class: "id-display-md",
                            if my_id.is_empty() { "—" } else { "{format_local_id(&my_id)}" }
                        }
                        button {
                            class: "btn-outline-accent",
                            onclick: {
                                let id = local_id();
                                move |_| { let _ = set_system_clipboard(&id); }
                            },
                            "{copy_label}"
                        }
                    }
                }

                div {
                    class: "value-row",
                    div { class: "label-sm", "{tr(keys::HOST_LABEL_PW, l)}" }
                    div {
                        class: "flex-center-row",
                        span {
                            class: "text-pri text-16 fw-600",
                            style: "letter-spacing: 3px;",
                            if host_password().is_empty() { "{pw_open}" } else { "{host_password()}" }
                        }
                        if !host_password().is_empty() {
                            button {
                                class: "btn-outline-accent",
                                onclick: {
                                    let pwd = host_password();
                                    move |_| { let _ = set_system_clipboard(&pwd); }
                                },
                                "{copy_label}"
                            }
                        }
                    }
                }
            }

            // ── Password settings ──
            div {
                class: "card mb-16",
                div { class: "label-sm mb-12", "{tr(keys::HOST_SECTION_PW, l)}" }
                div {
                    class: "host-btn-row",
                    div {
                        class: "flex-1",
                        label {
                            class: "label-sm connect-form-label",
                            "{tr(keys::HOST_FIELD_PW, l)}"
                        }
                        input {
                            r#type: "password",
                            class: "input",
                            placeholder: "{tr(keys::HOST_PLACEHOLDER_PW, l)}",
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
                                    // Update env file if systemd service is installed
                                    if si() {
                                        update_systemd_env(&id, &host_password(), &stun, 15);
                                    }
                                }
                            },
                        }
                    }
                    button {
                        class: "btn-ghost",
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
                                if si() {
                                    update_systemd_env(&id, &host_password(), &stun, 15);
                                }
                            }
                        },
                        "{tr(keys::HOST_BTN_RANDOM, l)}"
                    }
                    button {
                        class: "btn-outline-danger",
                        onclick: {
                            let si = svc_installed;
                            let id = id_for_pw_clear.clone();
                            let stun = crate::config::load_stun_servers();
                            move |_| {
                                host_password.set(String::new());
                                let mut cfg = load_config();
                                cfg.host_password = String::new();
                                save_config(&cfg);
                                if si() {
                                    update_systemd_env(&id, "", &stun, 15);
                                }
                            }
                        },
                        "{tr(keys::HOST_BTN_CLEAR, l)}"
                    }
                }
                div {
                    class: "text-dim text-11",
                    style: "margin-top: 8px; line-height: 1.5;",
                    "{tr(keys::HOST_PW_HELP, l)}"
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
                        div {
                            class: "info-panel",
                            div {
                                class: "text-accent text-12 fw-600 mb-6",
                                "{tr(keys::HOST_RUNNING_TITLE, l)}"
                            }
                            div {
                                class: "text-sec text-12",
                                style: "line-height: 1.6;",
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
