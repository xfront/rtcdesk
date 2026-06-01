//! Mouse event coordinate mapping and button helpers.

use dioxus::prelude::*;
use dioxus_html::input_data::MouseButton;
use tokio::sync::mpsc;

use shared::frame;
use crate::quality::ScaleMode;
use crate::remote::viewer_loop::InputCmd;

// ─── Mouse action ────────────────────────────────────────────────────────────

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum MouseAction {
    Down,
    Up,
    Move,
}

// ─── Public handler ──────────────────────────────────────────────────────────

pub fn handle_mouse_event(
    evt: &Event<MouseData>,
    action: MouseAction,
    win: &dioxus::desktop::DesktopService,
    orig_w: u32,
    orig_h: u32,
    frame_w: u32,
    frame_h: u32,
    scale_mode: ScaleMode,
    is_fullscreen: bool,
    tx: Option<mpsc::UnboundedSender<InputCmd>>,
) {
    let (mut ew, mut eh) = logical_size(win);
    // In non-fullscreen mode a 40px custom titlebar sits above the viewport
    // div, so the actual viewport height is window height minus titlebar.
    if !is_fullscreen {
        eh = (eh - frame::TITLEBAR_HEIGHT).max(1.0);
    }
    // In Original mode the inner div wraps the image at natural frame size,
    // so element coordinates map 1:1 to frame pixels.
    if scale_mode == ScaleMode::Original {
        ew = frame_w as f64;
        eh = frame_h as f64;
    }
    let (rx, ry) =
        relative_coords(evt, orig_w, orig_h, frame_w, frame_h, scale_mode, ew, eh);
    let btn = if action == MouseAction::Move {
        frame::BTN_MOVE
    } else {
        mouse_button(evt)
    };
    if let Some(tx) = tx {
        let _ = tx.send(InputCmd::Mouse {
            x: rx,
            y: ry,
            button: btn,
            pressed: action == MouseAction::Down,
        });
    }
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn mouse_button(evt: &Event<MouseData>) -> u8 {
    evt.data()
        .trigger_button()
        .map(|b| match b {
            MouseButton::Primary   => frame::BTN_LEFT,
            MouseButton::Auxiliary => frame::BTN_MIDDLE,
            MouseButton::Secondary => frame::BTN_RIGHT,
            _                      => frame::BTN_LEFT,
        })
        .unwrap_or(frame::BTN_LEFT)
}

fn relative_coords(
    evt: &Event<MouseData>,
    orig_w: u32,
    orig_h: u32,
    frame_w: u32,
    frame_h: u32,
    scale: ScaleMode,
    elem_w: f64,
    elem_h: f64,
) -> (f64, f64) {
    let ec = evt.data().element_coordinates();
    let (cx, cy) = (ec.x as f64, ec.y as f64);
    let (fw, fh, ow, oh) =
        (frame_w as f64, frame_h as f64, orig_w as f64, orig_h as f64);

    if elem_w <= 1.0 || elem_h <= 1.0 || fw <= 1.0 || fh <= 1.0 {
        return (cx.clamp(0.0, ow), cy.clamp(0.0, oh));
    }

    let (fx, fy) = match scale {
        ScaleMode::Fit => {
            let aspect = fw / fh;
            let elem_aspect = elem_w / elem_h;
            let (img_w, img_h, off_x, off_y) = if elem_aspect > aspect {
                let h = elem_h;
                let w = h * aspect;
                (w, h, (elem_w - w) / 2.0, 0.0)
            } else {
                let w = elem_w;
                let h = w / aspect;
                (w, h, 0.0, (elem_h - h) / 2.0)
            };
            (
                ((cx - off_x) / img_w * fw).clamp(0.0, fw),
                ((cy - off_y) / img_h * fh).clamp(0.0, fh),
            )
        }
        ScaleMode::Stretch => (
            (cx / elem_w * fw).clamp(0.0, fw),
            (cy / elem_h * fh).clamp(0.0, fh),
        ),
        ScaleMode::Original => {
            // Inner div wraps the image at natural frame size, so
            // element coordinates correspond directly to frame pixels.
            (cx.clamp(0.0, fw), cy.clamp(0.0, fh))
        }
    };

    let (scale_x, scale_y) = (ow / fw, oh / fh);
    (
        (fx * scale_x).clamp(0.0, ow),
        (fy * scale_y).clamp(0.0, oh),
    )
}

pub fn logical_size(window: &dioxus::desktop::DesktopService) -> (f64, f64) {
    let s = window.inner_size();
    let sf = window.scale_factor().max(frame::MIN_SCALE_FACTOR);
    ((s.width as f64) / sf, (s.height as f64) / sf)
}

// ─── CSS helpers (viewport display) ─────────────────────────────────────────

pub fn frame_display_style(scale: ScaleMode, fw: u32, fh: u32) -> String {
    match scale {
        ScaleMode::Fit | ScaleMode::Stretch => {
            "display:flex;align-items:center;justify-content:center;width:100%;height:100%;"
                .to_string()
        }
        ScaleMode::Original => {
            format!(
                "display:block;flex-shrink:0;width:{fw}px;height:{fh}px;",
            )
        }
    }
}

pub fn image_style(scale: ScaleMode, fw: u32, fh: u32) -> String {
    match scale {
        ScaleMode::Fit => {
            "width:100%;height:100%;object-fit:contain;pointer-events:none;user-select:none;"
                .to_string()
        }
        ScaleMode::Stretch => {
            "width:100%;height:100%;object-fit:fill;pointer-events:none;user-select:none;"
                .to_string()
        }
        ScaleMode::Original => {
            format!(
                "width:{fw}px;height:{fh}px;pointer-events:none;user-select:none;",
            )
        }
    }
}
