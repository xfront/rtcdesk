//! Home tab: Device list layout with search, groups, and detail panel.
//! Refactored with dioxus-bootstrap-css components.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use dioxus_bootstrap_css::theme::Theme;
use crate::Page;

// ─── Device data model ────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub struct DeviceItem {
    pub id: String,
    pub name: String,
    pub alias: Option<String>,
    pub os: String,
    pub online: bool,
    pub is_local: bool,
}

impl DeviceItem {
    fn display_name(&self) -> String {
        self.alias.clone().unwrap_or_else(|| self.name.clone())
    }
}

#[derive(Clone, PartialEq)]
struct DeviceGroup {
    label: String,
    devices: Vec<DeviceItem>,
    expanded: bool,
}

// ─── Home tab ────────────────────────────────────────────────────────────────

#[component]
pub fn HomeTab(
    page: Signal<Page>,
    server_addr: Signal<String>,
    remote_id: Signal<String>,
    quality: Signal<usize>,
    theme: Signal<Theme>,
    locale: Signal<String>,
    local_id: Signal<String>,
    password: Signal<String>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let my_id = local_id();

    let mut search_text = use_signal(String::new);
    let mut selected_id = use_signal(|| Option::<String>::None);

    let all_devices = use_memo(move || build_device_list(&my_id, &l));

    let filtered_devices = use_memo(move || {
        let query = search_text().to_lowercase();
        if query.is_empty() { return all_devices(); }
        all_devices()
            .into_iter()
            .filter(|d| d.display_name().to_lowercase().contains(&query)
                || d.id.to_lowercase().contains(&query))
            .collect::<Vec<_>>()
    });

    let groups = use_memo(move || {
        let devs = filtered_devices();
        vec![
            DeviceGroup { label: tr(keys::HOME_GROUP_MY, l).to_string(), devices: devs, expanded: true },
        ]
    });

    rsx! {
        div {
            class: "d-flex flex-grow-1 h-100 overflow-hidden",

            // Left: device list
            div {
                class: "flex-grow-1 min-w-0 d-flex flex-column overflow-y-auto",
                style: "padding: 16px 20px; border-right: 1px solid var(--bs-border-color);",

                // Search row
                div { class: "d-flex gap-2 mb-3 flex-shrink-0",
                    InputGroup {
                        class: "flex-grow-1",
                        span { class: "input-group-text",
                            svg {
                                xmlns: "http://www.w3.org/2000/svg",
                                width: "14", height: "14",
                                fill: "none", stroke: "var(--bs-tertiary-color)", stroke_width: "1.5",
                                path { d: "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM13 13l-3-3" }
                            }
                        }
                        Input {
                            class: "border-0 bg-transparent shadow-none",
                            r#type: "text",
                            placeholder: tr(keys::HOME_SEARCH_PLACEHOLDER, l),
                            value: "{search_text()}",
                            oninput: move |e: FormEvent| search_text.set(e.value()),
                        }
                        if !search_text().is_empty() {
                            Button {
                                color: Color::Secondary,
                                outline: true,
                                onclick: move |_| search_text.set(String::new()),
                                "\u{2715}"
                            }
                        }
                    }
                    Button {
                        color: Color::Primary,
                        outline: true,
                        title: tr(keys::HOME_ADD_DEVICE, l),
                        onclick: move |_| {},
                        "+"
                    }
                }

                // Device groups
                div { class: "d-flex flex-column gap-1 flex-grow-1",
                    for (g_idx, grp) in groups().iter().enumerate() {
                        DeviceGroupWidget {
                            key: "{g_idx}",
                            group: grp.clone(),
                            selected_id: selected_id(),
                            on_select: move |id| selected_id.set(Some(id)),
                            on_connect: handle_device_connect(page, server_addr, remote_id, password),
                            locale: l.to_string(),
                        }
                    }
                }
            }

            // Right: detail panel
            if let Some(sid) = selected_id() {
                DeviceDetailPanel {
                    devices: filtered_devices(),
                    selected_id: sid,
                    _theme: theme,
                    locale: l.to_string(),
                }
            } else {
                div {
                    class: "d-flex flex-column flex-shrink-0 overflow-y-auto",
                    style: "width: 280px; min-width: 240px; max-width: 320px;
                            background: var(--bs-tertiary-bg);
                            border-left: 1px solid var(--bs-border-color);",
                    div {
                        class: "flex-grow-1 d-flex align-items-center justify-content-center small pt-5",
                        style: "color: var(--bs-tertiary-color);",
                        "{tr(keys::HOME_SELECT_HINT, l)}"
                    }
                }
            }
        }
    }
}

fn handle_device_connect(
    mut page: Signal<Page>,
    server_addr: Signal<String>,
    mut remote_id: Signal<String>,
    mut password: Signal<String>,
) -> EventHandler<String> {
    EventHandler::new(move |id: String| {
        remote_id.set(id.clone());
        let mut cfg = load_config();
        cfg.remote_id = id;
        cfg.server_addr = server_addr();
        save_config(&cfg);
        password.set(String::new());
        page.set(Page::Remote);
    })
}

// ─── Device group widget ──────────────────────────────────────────────────

#[component]
fn DeviceGroupWidget(
    group: DeviceGroup,
    selected_id: Option<String>,
    on_select: EventHandler<String>,
    on_connect: EventHandler<String>,
    locale: String,
) -> Element {
    let mut expanded = use_signal(|| group.expanded);
    let dev_count = group.devices.len();
    let l: &'static str = Box::leak(locale.into_boxed_str());

    rsx! {
        div {
            Button {
                color: Color::Light,
                outline: true,
                class: "border-0 text-start w-100 d-flex align-items-center gap-2 py-2 px-3",
                onclick: move |_| expanded.toggle(),
                span {
                    style: "transition: transform 0.15s; display: inline-block; font-size: 10px;",
                    if expanded() { "\u{25BC}" } else { "\u{25B6}" }
                }
                span { style: "font-weight: 600;", "{group.label}" }
                Badge { color: Color::Secondary, class: "ms-1", "{dev_count}" }
            }
            if expanded() && !group.devices.is_empty() {
                div { class: "ps-3 pb-1",
                    for dev in &group.devices {
                        {
                            let did = dev.id.clone();
                            let sel = selected_id.as_ref().map_or(false, |s| *s == did);
                            rsx! {
                                DeviceRow {
                                    device: dev.clone(),
                                    selected: sel,
                                    on_select: move |id| on_select.call(id),
                                    on_connect: move |id| on_connect.call(id),
                                    locale: l.to_string(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─── Single device row ───────────────────────────────────────────────────

#[component]
fn DeviceRow(
    device: DeviceItem,
    selected: bool,
    on_select: EventHandler<String>,
    on_connect: EventHandler<String>,
    locale: String,
) -> Element {
    let l: &'static str = Box::leak(locale.into_boxed_str());
    let dot_style = if device.online {
        "background: var(--bs-success); box-shadow: 0 0 6px var(--bs-success);"
    } else {
        "background: var(--bs-tertiary-color);"
    };

    let os_icon = match device.os.as_str() {
        s if s.contains("linux") => "\u{1F427}",
        s if s.contains("mac") || s.contains("darwin") => "\u{1F34E}",
        s if s.contains("windows") => "\u{1F5A5}",
        _ => "\u{1F4BB}",
    };

    let sel_id = device.id.clone();
    let con_id = device.id.clone();
    let badge_text = tr(keys::HOME_BADGE_LOCAL, l);
    let row_bg = if selected { "rgba(var(--bs-success-rgb), 0.12)" } else { "transparent" };

    rsx! {
        button {
            class: "btn border-0 text-start w-100 d-flex align-items-center gap-2 py-2 px-3",
            style: "background: {row_bg}; color: var(--bs-body-color);",
            onclick: move |_| on_select.call(sel_id.clone()),
            ondoubleclick: move |_| on_connect.call(con_id.clone()),
            div {
                class: "rounded-circle flex-shrink-0",
                style: "width: 7px; height: 7px; margin-top: 1px; {dot_style}",
            }
            div { class: "d-flex align-items-center gap-1 flex-grow-1 min-w-0",
                span { class: "flex-shrink-0", "{os_icon}" }
                span { class: "text-truncate fw-medium", "{device.display_name()}" }
                if device.is_local {
                    Badge {
                        color: Color::Secondary,
                        style: "flex-shrink: 0; white-space: nowrap;",
                        "{badge_text}"
                    }
                }
            }
        }
    }
}

// ─── Right detail panel ──────────────────────────────────────────────────

#[component]
fn DeviceDetailPanel(
    devices: Vec<DeviceItem>,
    selected_id: String,
    _theme: Signal<Theme>,
    locale: String,
) -> Element {
    let l: &'static str = Box::leak(locale.into_boxed_str());

    let device = devices.iter().find(|d| d.id == selected_id).cloned();
    let alias_text = device.as_ref()
        .and_then(|d| d.alias.clone())
        .unwrap_or_else(|| tr(keys::HOME_DETAIL_SET_ALIAS, l).to_string());

    match device {
        Some(dev) => rsx! {
            div {
                class: "d-flex flex-column flex-shrink-0 overflow-y-auto",
                style: "width: 280px; min-width: 240px; max-width: 320px;
                        background: var(--bs-tertiary-bg);
                        border-left: 1px solid var(--bs-border-color);",

                div {
                    class: "d-flex align-items-center justify-content-between px-3 pt-3 pb-2 flex-shrink-0",
                    span { class: "fw-semibold", "{tr(keys::HOME_DETAIL_TITLE, l)}" }
                Button {
                    size: Size::Sm,
                    class: "border-0 text-secondary",
                    "\u{2026}"
                }
                }

                Card { class: "border-0 bg-transparent mx-3 mb-3",
                    body: rsx! {
                        div { class: "d-flex justify-content-between py-1",
                            small { class: "text-secondary", "{tr(keys::HOME_DETAIL_NAME, l)}" }
                            div { class: "d-flex align-items-center gap-1 fw-medium", "{dev.display_name()}" }
                        }
                        div { class: "d-flex justify-content-between py-1",
                            small { class: "text-secondary", "{tr(keys::HOME_DETAIL_ALIAS, l)}" }
                            div { class: "d-flex align-items-center gap-1",
                                small { style: "color: var(--bs-secondary-color);", "{alias_text}" }
                                Button { size: Size::Sm, class: "border-0 p-0 small text-secondary", "\u{270E}" }
                            }
                        }
                    }
                }

                if dev.is_local {
                    Card { class: "border-0 bg-transparent mx-3",
                        body: rsx! {
                            h6 { class: "mb-2", "{tr(keys::HOME_DETAIL_UNATTENDED, l)}" }
                            div { class: "d-flex justify-content-between py-1",
                                small { class: "text-secondary", "{tr(keys::HOME_DETAIL_SYS_AUTH, l)}" }
                                small { class: "text-danger", "{tr(keys::HOME_DETAIL_SET, l)}" }
                            }
                            div { class: "d-flex justify-content-between py-1",
                                small { class: "text-secondary", "{tr(keys::HOME_DETAIL_PW_AUTH, l)}" }
                                small { class: "text-danger", "{tr(keys::HOME_DETAIL_UNAUTH, l)}" }
                            }
                            div { class: "d-flex justify-content-between py-1",
                                small { class: "text-secondary", "{tr(keys::HOME_DETAIL_AUTOSTART, l)}" }
                                small { class: "text-danger", "{tr(keys::HOME_DETAIL_ACTIVE, l)}" }
                            }
                        }
                    }
                }
            }
        },
        None => rsx! {
            div {
                class: "d-flex align-items-center justify-content-center flex-shrink-0 small",
                style: "width: 280px; min-width: 240px; max-width: 320px;
                        background: var(--bs-tertiary-bg); border-left: 1px solid var(--bs-border-color);
                        color: var(--bs-tertiary-color); padding-top: 60px;",
                "{tr(keys::HOME_DETAIL_NOT_FOUND, l)}"
            }
        },
    }
}

// ─── Helper: build device list from config ──────────────────────────────────

fn build_device_list(my_id: &str, l: &str) -> Vec<DeviceItem> {
    let mut result = Vec::new();

    result.push(DeviceItem {
        id: my_id.to_string(),
        name: tr(keys::HOME_LOCAL_NAME, l).to_string(),
        alias: None,
        os: std::env::consts::OS.to_string(),
        online: true,
        is_local: true,
    });

    let cfg = load_config();
    let mid = my_id.to_string();
    for entry in &cfg.connection_history {
        if entry.remote_id != mid {
            result.push(DeviceItem {
                id: entry.remote_id.clone(),
                name: format_local_id(&entry.remote_id),
                alias: if entry.alias.is_empty() { None } else { Some(entry.alias.clone()) },
                os: String::new(),
                online: false,
                is_local: false,
            });
        }
    }

    result
}
