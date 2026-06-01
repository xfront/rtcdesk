//! Invisible resize handles at window edges for custom-decorated windows.

use dioxus::desktop::use_window;
use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::prelude::*;

#[component]
pub fn ResizeEdges() -> Element {
    let window = use_window();
    let (w_top, w_bot, w_l, w_r, w_tl, w_tr, w_bl, w_br) = (
        window.clone(), window.clone(), window.clone(), window.clone(),
        window.clone(), window.clone(), window.clone(), window.clone(),
    );

    let edge = "position:fixed;z-index:9999;";
    rsx! {
        div { style: "{edge}top:0;left:8px;right:8px;height:4px;cursor:n-resize;",
            onmousedown: move |_| {
                let _ = w_top.drag_resize_window(ResizeDirection::North);
            }
        }
        div { style: "{edge}bottom:0;left:8px;right:8px;height:4px;cursor:s-resize;",
            onmousedown: move |_| {
                let _ = w_bot.drag_resize_window(ResizeDirection::South);
            }
        }
        div { style: "{edge}top:4px;left:0;bottom:4px;width:4px;cursor:w-resize;",
            onmousedown: move |_| {
                let _ = w_l.drag_resize_window(ResizeDirection::West);
            }
        }
        div { style: "{edge}top:4px;right:0;bottom:4px;width:4px;cursor:e-resize;",
            onmousedown: move |_| {
                let _ = w_r.drag_resize_window(ResizeDirection::East);
            }
        }

        // Corners — wider grab area for diagonal resize
        div { style: "{edge}top:0;left:0;width:8px;height:8px;cursor:nw-resize;",
            onmousedown: move |_| {
                let _ = w_tl.drag_resize_window(ResizeDirection::NorthWest);
            }
        }
        div { style: "{edge}top:0;right:0;width:8px;height:8px;cursor:ne-resize;",
            onmousedown: move |_| {
                let _ = w_tr.drag_resize_window(ResizeDirection::NorthEast);
            }
        }
        div { style: "{edge}bottom:0;left:0;width:8px;height:8px;cursor:sw-resize;",
            onmousedown: move |_| {
                let _ = w_bl.drag_resize_window(ResizeDirection::SouthWest);
            }
        }
        div { style: "{edge}bottom:0;right:0;width:8px;height:8px;cursor:se-resize;",
            onmousedown: move |_| {
                let _ = w_br.drag_resize_window(ResizeDirection::SouthEast);
            }
        }
    }
}
