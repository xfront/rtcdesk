//! Theme definitions: color tokens, ThemeColors struct, tc() accessor,
//! and apply_theme() for CSS variable switching.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

// ─── Theme enum ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Copy, Serialize, Deserialize)]
pub enum Theme {
    Dark,
    Light,
    SkyBlue,
    GrassGreen,
    Orange,
    Purple,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Dark
    }
}

// ─── Color token struct ─────────────────────────────────────────────────────

pub struct ThemeColors {
    pub bg_base:      &'static str,
    pub bg_sidebar:   &'static str,
    pub bg_card:      &'static str,
    pub bg_hover:     &'static str,
    pub bg_input:     &'static str,
    pub border:       &'static str,
    pub border_focus: &'static str,
    pub accent:       &'static str,
    pub accent_dim:   &'static str,
    pub danger:       &'static str,
    pub warn:         &'static str,
    pub text_pri:     &'static str,
    pub text_sec:     &'static str,
    pub text_dim:     &'static str,
    pub toolbar:      &'static str,
    pub btn_text:     &'static str,
}

// ─── Dark theme ─────────────────────────────────────────────────────────────

pub static DARK: ThemeColors = ThemeColors {
    bg_base:      "#0f1116",
    bg_sidebar:   "#161b22",
    bg_card:      "#1c2128",
    bg_hover:     "#21262d",
    bg_input:     "#0d1117",
    border:       "#30363d",
    border_focus: "#388bfd",
    accent:       "#3fb950",
    accent_dim:   "#238636",
    danger:       "#f85149",
    warn:         "#d29922",
    text_pri:     "#e6edf3",
    text_sec:     "#8b949e",
    text_dim:     "#484f58",
    toolbar:      "rgba(22, 27, 34, 0.95)",
    btn_text:     "#0d1117",
};

// ─── Light theme ─────────────────────────────────────────────────────────────

pub static LIGHT: ThemeColors = ThemeColors {
    bg_base:      "#f6f8fa",
    bg_sidebar:   "#ffffff",
    bg_card:      "#ffffff",
    bg_hover:     "#f3f4f6",
    bg_input:     "#f6f8fa",
    border:       "#d0d7de",
    border_focus: "#0969da",
    accent:       "#1a7f37",
    accent_dim:   "#116329",
    danger:       "#cf222e",
    warn:         "#9a6700",
    text_pri:     "#1f2328",
    text_sec:     "#57606a",
    text_dim:     "#8c959f",
    toolbar:      "rgba(246, 248, 250, 0.95)",
    btn_text:     "#ffffff",
};

// ─── Sky Blue theme (dark background, blue accent) ───────────────────────────

pub static SKY_BLUE: ThemeColors = ThemeColors {
    bg_base:      "#0a1628",
    bg_sidebar:   "#0f1f3a",
    bg_card:      "#152240",
    bg_hover:     "#1a2d50",
    bg_input:     "#0d1a30",
    border:       "#224060",
    border_focus: "#4dabf7",
    accent:       "#4dabf7",
    accent_dim:   "#339af0",
    danger:       "#f85149",
    warn:         "#f08c00",
    text_pri:     "#e6f0ff",
    text_sec:     "#7a9ec9",
    text_dim:     "#3d5477",
    toolbar:      "rgba(15, 31, 58, 0.95)",
    btn_text:     "#0a1628",
};

// ─── Grass Green theme (light background, green accent) ──────────────────────

pub static GRASS_GREEN: ThemeColors = ThemeColors {
    bg_base:      "#f4faf0",
    bg_sidebar:   "#ffffff",
    bg_card:      "#ffffff",
    bg_hover:     "#e8f5e0",
    bg_input:     "#f6fbf2",
    border:       "#c0d9b0",
    border_focus: "#40c057",
    accent:       "#2f9e44",
    accent_dim:   "#2b8a3e",
    danger:       "#e03131",
    warn:         "#e8590c",
    text_pri:     "#1f2a18",
    text_sec:     "#5c7660",
    text_dim:     "#8fa98a",
    toolbar:      "rgba(244, 250, 240, 0.95)",
    btn_text:     "#ffffff",
};

// ─── Orange theme (dark warm background, orange accent) ──────────────────────

pub static ORANGE: ThemeColors = ThemeColors {
    bg_base:      "#1c1410",
    bg_sidebar:   "#241a14",
    bg_card:      "#2a1e16",
    bg_hover:     "#33241a",
    bg_input:     "#1e1510",
    border:       "#3d2b1a",
    border_focus: "#ff8c42",
    accent:       "#ff8c42",
    accent_dim:   "#e67830",
    danger:       "#ff5757",
    warn:         "#f0a030",
    text_pri:     "#fff0e8",
    text_sec:     "#bf9a7a",
    text_dim:     "#6b4d33",
    toolbar:      "rgba(36, 26, 20, 0.95)",
    btn_text:     "#1c1410",
};

// ─── Purple theme (dark purple background, violet accent) ────────────────────

pub static PURPLE: ThemeColors = ThemeColors {
    bg_base:      "#14111c",
    bg_sidebar:   "#1b1726",
    bg_card:      "#211c2e",
    bg_hover:     "#2a243a",
    bg_input:     "#16121e",
    border:       "#3a2f4a",
    border_focus: "#a78bfa",
    accent:       "#a78bfa",
    accent_dim:   "#8b5cf6",
    danger:       "#f87171",
    warn:         "#fbbf24",
    text_pri:     "#f0ebff",
    text_sec:     "#a094c4",
    text_dim:     "#5a4e7a",
    toolbar:      "rgba(27, 23, 38, 0.95)",
    btn_text:     "#14111c",
};

// ─── Accessor ────────────────────────────────────────────────────────────────

/// Return the color token set for the given theme.
#[inline]
pub fn tc(theme: Theme) -> &'static ThemeColors {
    match theme {
        Theme::Dark       => &DARK,
        Theme::Light      => &LIGHT,
        Theme::SkyBlue    => &SKY_BLUE,
        Theme::GrassGreen => &GRASS_GREEN,
        Theme::Orange     => &ORANGE,
        Theme::Purple     => &PURPLE,
    }
}

// ─── CSS theme application ───────────────────────────────────────────────────

/// Set the `data-theme` attribute on `<html>` so CSS variables switch instantly.
/// Call this on app startup and whenever the theme signal changes.
pub fn apply_theme(theme: Theme) {
    let theme_str = match theme {
        Theme::Dark       => "dark",
        Theme::Light      => "light",
        Theme::SkyBlue    => "sky-blue",
        Theme::GrassGreen => "grass-green",
        Theme::Orange     => "orange",
        Theme::Purple     => "purple",
    };
    let _ = document::eval(&format!(
        "document.documentElement.setAttribute('data-theme', '{}');",
        theme_str
    ));
}
