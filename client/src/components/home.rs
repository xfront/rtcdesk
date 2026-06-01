//! Home tab: Device list layout with search, groups, and detail panel.

use dioxus::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use crate::theme::{tc, Theme};
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
    let _t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let my_id = local_id();
    let my_id_clone = my_id.clone();

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
        div { class: "device-list-page",

            div { class: "device-list-area",
                div { class: "device-search-row",
                    div { class: "device-search-input-wrap",
                        svg_icon_search {}
                        input {
                            r#type: "text",
                            class: "device-search-input",
                            placeholder: "{tr(keys::HOME_SEARCH_PLACEHOLDER, l)}",
                            value: "{search_text()}",
                            oninput: move |e| search_text.set(e.value()),
                        }
                        if !search_text().is_empty() {
                            button {
                                class: "device-search-clear",
                                onclick: move |_| search_text.set(String::new()),
                                "\u{2715}"
                            }
                        }
                    }
                    button {
                        class: "device-search-add",
                        title: tr(keys::HOME_ADD_DEVICE, l),
                        onclick: move |_| {},
                        "+"
                    }
                }

                div { class: "device-groups",
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

            if let Some(sid) = selected_id() {
                DeviceDetailPanel {
                    devices: filtered_devices(),
                    selected_id: sid,
                    theme,
                    locale: l.to_string(),
                }
            } else {
                div { class: "device-detail-panel",
                    div { class: "device-detail-empty", span { "{tr(keys::HOME_SELECT_HINT, l)}" } }
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

fn svg_icon_search() -> Element {
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            width: "14", height: "14",
            fill: "none", stroke: "var(--text-dim)", stroke_width: "1.5",
            path { d: "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM13 13l-3-3" }
        }
    }
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
        div { class: "device-group",
            button {
                class: "device-group-header",
                onclick: move |_| expanded.toggle(),
                span {
                    style: "transition: transform 0.15s; display:inline-block; font-size:10px; margin-right:4px;",
                    if expanded() { "\u{25BC}" } else { "\u{25B6}" }
                }
                span { style: "font-weight: 600; color: var(--text-pri);", "{group.label}" }
                span { style: "color: var(--text-dim); font-size: 12px;", "({dev_count})" }
            }
            if expanded() && !group.devices.is_empty() {
                div { class: "device-group-body",
                    for dev in &group.devices {
                        {
                            let did = dev.id.clone();
                            let sel = match &selected_id {
                                Some(s) if *s == did => true,
                                _ => false,
                            };
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
    let row_class = if selected { "device-row-selected" } else { "device-row" };
    let dot_style = if device.online {
        "background: var(--accent); box-shadow: 0 0 6px var(--accent);"
    } else {
        "background: var(--text-dim);"
    };
    let device_id = device.id.clone();

    let os_icon = match device.os.as_str() {
        s if s.contains("linux") => "\u{1F427}",
        s if s.contains("mac") || s.contains("darwin") => "\u{1F34E}",
        s if s.contains("windows") => "\u{1F5A5}",
        _ => "\u{1F4BB}",
    };

    let sel_id = device_id.clone();
    let con_id = device_id.clone();
    let badge_text = tr(keys::HOME_BADGE_LOCAL, l);

    rsx! {
        button {
            class: row_class,
            onclick: move |_| on_select.call(sel_id.clone()),
            ondoubleclick: move |_| on_connect.call(con_id.clone()),
            div {
                class: "device-dot",
                style: "{dot_style}",
            }
            div { class: "device-name-cell",
                span { style: "font-size: 14px; margin-right: 6px; flex-shrink: 0;", "{os_icon}" }
                span { class: "truncate fw-500", "{device.display_name()}" }
                if device.is_local {
                    span { class: "device-local-badge", "{badge_text}" }
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
    theme: Signal<Theme>,
    locale: String,
) -> Element {
    let _t = tc(theme());
    let l: &'static str = Box::leak(locale.into_boxed_str());

    let device = devices.iter().find(|d| d.id == selected_id).cloned();
    let alias_text = device.as_ref()
        .and_then(|d| d.alias.clone())
        .unwrap_or_else(|| tr(keys::HOME_DETAIL_SET_ALIAS, l).to_string());

    match device {
        Some(dev) => rsx! {
            div { class: "device-detail-panel",

                div { class: "device-detail-header",
                    span { class: "fw-600 text-14", "{tr(keys::HOME_DETAIL_TITLE, l)}" }
                    button {
                        style: "color:var(--text-dim);font-size:16px;background:none;border:none;cursor:pointer;padding:4px;",
                        "\u{2026}"
                    }
                }

                div { class: "device-detail-section",
                    div { class: "device-detail-row",
                        span { class: "device-detail-label", "{tr(keys::HOME_DETAIL_NAME, l)}" }
                        div { class: "device-detail-value",
                            span { "{dev.display_name()}" }
                            button { class: "device-detail-edit-icon", "\u{270E}" }
                        }
                    }
                    div { class: "device-detail-row",
                        span { class: "device-detail-label", "{tr(keys::HOME_DETAIL_ALIAS, l)}" }
                        div { class: "device-detail-value",
                            span {
                                style: "color: var(--text-sec);",
                                "{alias_text}"
                            }
                            button { class: "device-detail-edit-icon", "\u{270E}" }
                        }
                    }
                }

                if dev.is_local {
                    div { class: "device-detail-section",
                        span { class: "device-detail-subtitle", "{tr(keys::HOME_DETAIL_UNATTENDED, l)}" }

                        div { class: "device-detail-row",
                            span { class: "device-detail-label-sm", "{tr(keys::HOME_DETAIL_SYS_AUTH, l)}" }
                            span { class: "status-set", "{tr(keys::HOME_DETAIL_SET, l)}" }
                        }
                        div { class: "device-detail-row",
                            span { class: "device-detail-label-sm", "{tr(keys::HOME_DETAIL_PW_AUTH, l)}" }
                            span { class: "status-unset", "{tr(keys::HOME_DETAIL_UNAUTH, l)}" }
                        }
                        div { class: "device-detail-row",
                            span { class: "device-detail-label-sm", "{tr(keys::HOME_DETAIL_AUTOSTART, l)}" }
                            span { class: "status-active", "{tr(keys::HOME_DETAIL_ACTIVE, l)}" }
                        }
                    }
                }
            }
        },
        None => rsx! {
            div { class: "device-detail-panel",
                div { class: "device-detail-empty", span { "{tr(keys::HOME_DETAIL_NOT_FOUND, l)}" } }
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
