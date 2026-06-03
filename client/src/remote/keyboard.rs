//! Keyboard event mapping and local hotkey handling.

use dioxus::prelude::*;

use shared::frame;
use crate::WindowCmd;

/// Map a Dioxus keyboard event to a virtual key code compatible with the host.
pub fn key_event_to_code(evt: &Event<KeyboardData>) -> u32 {
    match evt.data().code() {
        Code::Backspace                            => frame::VK_BACK,
        Code::Tab                                  => frame::VK_TAB,
        Code::Enter | Code::NumpadEnter            => frame::VK_RETURN,
        Code::Escape                               => frame::VK_ESCAPE,
        Code::Space                                => frame::VK_SPACE,
        Code::Delete                               => frame::VK_DELETE,
        Code::Insert                               => frame::VK_INSERT,
        Code::Home                                 => frame::VK_HOME,
        Code::End                                  => frame::VK_END,
        Code::PageUp                               => frame::VK_PRIOR,
        Code::PageDown                             => frame::VK_NEXT,
        Code::ArrowLeft                            => frame::VK_LEFT,
        Code::ArrowUp                              => frame::VK_UP,
        Code::ArrowRight                           => frame::VK_RIGHT,
        Code::ArrowDown                            => frame::VK_DOWN,
        Code::ShiftLeft | Code::ShiftRight         => frame::VK_SHIFT,
        Code::ControlLeft | Code::ControlRight     => frame::VK_CONTROL,
        Code::AltLeft | Code::AltRight             => frame::VK_ALT,
        Code::MetaLeft | Code::MetaRight           => frame::VK_META,
        Code::CapsLock                             => frame::VK_CAPITAL,
        Code::NumLock                              => frame::VK_NUMLOCK,
        Code::ScrollLock                           => frame::VK_SCROLL,
        Code::Pause                                => frame::VK_PAUSE,
        Code::PrintScreen                          => frame::VK_PRINT,
        Code::F1  => frame::VK_F1,
        Code::F2  => frame::VK_F2,
        Code::F3  => frame::VK_F3,
        Code::F4  => frame::VK_F4,
        Code::F5  => frame::VK_F5,
        Code::F6  => frame::VK_F6,
        Code::F7  => frame::VK_F7,
        Code::F8  => frame::VK_F8,
        Code::F9  => frame::VK_F9,
        Code::F10 => frame::VK_F10,
        Code::F11 => frame::VK_F11,
        Code::F12 => frame::VK_F12,
        _ => match evt.data().key() {
            Key::Character(ref ch) if !ch.is_empty() => {
                ch.chars().next().unwrap() as u32
            }
            _ => 0,
        },
    }
}

/// Handle local (not forwarded) keydown events.
///
/// Returns `true` if the event was consumed locally.
pub fn handle_keydown_local(
    evt: &Event<KeyboardData>,
    mut right_ctrl: Signal<bool>,
    mut view_only: Signal<bool>,
    mut window_cmd: Signal<Vec<WindowCmd>>,
) -> bool {
    let key  = evt.data().key();
    let code = evt.data().code();
    if key == Key::Control && code == Code::ControlRight {
        right_ctrl.set(true);
        return true;
    }
    // Right Ctrl + F: toggle fullscreen
    if code == Code::KeyF
        && evt.data().modifiers().ctrl()
        && right_ctrl()
        && !evt.data().is_auto_repeating()
    {
        window_cmd.write().push(WindowCmd::ToggleFullscreen);
        return true;
    }
    // Right Ctrl + V: toggle view-only mode
    if code == Code::KeyV
        && right_ctrl()
        && !evt.data().is_auto_repeating()
    {
        let vo = !view_only();
        view_only.set(vo);
        return true;
    }
    false
}

/// Handle local keyup events.
///
/// Returns `true` if the event was consumed locally.
pub fn handle_keyup_local(
    evt: &Event<KeyboardData>,
    mut right_ctrl: Signal<bool>,
) -> bool {
    if evt.data().key() == Key::Control && evt.data().code() == Code::ControlRight {
        right_ctrl.set(false);
        return true;
    }
    false
}
