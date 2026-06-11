use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::i18n::keys;
use crate::i18n::t as tr;
use dioxus_bootstrap_css::theme::Theme;

/// Static lock password stored in memory for the session.
static LOCK_PASSWORD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[component]
pub fn LockUIDialog(
    locale: Signal<String>,
    is_locked: Signal<bool>,
    show_lock_dialog: Signal<bool>,
) -> Element {
    let l2: &'static str = Box::leak(locale().into_boxed_str());
    let currently_locked = is_locked();
    let modal_title = if currently_locked {
        tr(keys::SIDEBAR_LOCK_UNLOCK_TITLE, l2).to_string()
    } else {
        tr(keys::SIDEBAR_LOCK_SET_TITLE, l2).to_string()
    };

    let password_input = use_signal(String::new);
    let wrong_password = use_signal(|| false);
    let mut is_locked2 = is_locked;
    let mut show_lock2 = show_lock_dialog;
    let mut pwd_input2 = password_input;
    let mut wrong_pwd2 = wrong_password;

    rsx! {
        Modal {
            show: show_lock_dialog,
            title: modal_title,
            size: ModalSize::Sm,
            centered: true,
            backdrop_close: true,
            show_close: true,
            body: rsx! {
                div {
                    // Hint text
                    p {
                        class: "small text-secondary text-center mb-3",
                        if currently_locked {
                            "{tr(keys::SIDEBAR_LOCK_UNLOCK_HINT, l2)}"
                        } else {
                            "{tr(keys::SIDEBAR_LOCK_SET_HINT, l2)}"
                        }
                    }
                    // Password input
                    input {
                        r#type: "password",
                        class: "form-control form-control-sm text-center",
                        style: "border-radius: 8px; font-size: 14px;",
                        placeholder: if currently_locked {
                            tr(keys::SIDEBAR_LOCK_ENTER_PASSWORD, l2)
                        } else {
                            tr(keys::SIDEBAR_LOCK_SET_PASSWORD, l2)
                        },
                        value: "{pwd_input2}",
                        oninput: move |e| {
                            pwd_input2.set(e.value());
                            wrong_pwd2.set(false);
                        },
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter && !pwd_input2().is_empty() {
                                if currently_locked {
                                    let pwd = pwd_input2();
                                    let stored = LOCK_PASSWORD.lock().unwrap().clone();
                                    if stored.as_deref() == Some(pwd.as_str()) {
                                        *LOCK_PASSWORD.lock().unwrap() = None;
                                        is_locked2.set(false);
                                        show_lock2.set(false);
                                        pwd_input2.set(String::new());
                                    } else {
                                        wrong_pwd2.set(true);
                                    }
                                } else {
                                    *LOCK_PASSWORD.lock().unwrap() = Some(pwd_input2());
                                    is_locked2.set(true);
                                    show_lock2.set(false);
                                    pwd_input2.set(String::new());
                                }
                            }
                        },
                    }
                    // Wrong password message
                    if wrong_pwd2() {
                        p {
                            class: "small text-danger text-center mt-2 mb-0",
                            "{tr(keys::SIDEBAR_LOCK_WRONG_PASSWORD, l2)}"
                        }
                    }
                }
            },
            footer: rsx! {
                button {
                    class: if currently_locked { "btn btn-sm btn-warning" } else { "btn btn-sm btn-primary" },
                    style: "border-radius: 8px; font-size: 13px;",
                    disabled: pwd_input2().is_empty(),
                    onclick: move |_| {
                        if currently_locked {
                            let pwd = pwd_input2();
                            let stored = LOCK_PASSWORD.lock().unwrap().clone();
                            if stored.as_deref() == Some(pwd.as_str()) {
                                *LOCK_PASSWORD.lock().unwrap() = None;
                                is_locked2.set(false);
                                show_lock2.set(false);
                                pwd_input2.set(String::new());
                            } else {
                                wrong_pwd2.set(true);
                            }
                        } else {
                            *LOCK_PASSWORD.lock().unwrap() = Some(pwd_input2());
                            is_locked2.set(true);
                            show_lock2.set(false);
                            pwd_input2.set(String::new());
                        }
                    },
                    if currently_locked {
                        "{tr(keys::SIDEBAR_LOCK_UNLOCK, l2)}"
                    } else {
                        "{tr(keys::SIDEBAR_LOCK_NOW, l2)}"
                    }
                }
            },
        }
    }
}
