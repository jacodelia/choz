//! Shared color palette — adapted from seqterm's theme.
//!
//! All panels, modals, buttons, and overlays use these constants.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use ratatui::style::{Color, Style};

/// User-selected text colour (packed 0x00RRGGBB), read by every panel through
/// [`text`]. An atomic keeps it reachable from the draw code without threading
/// a settings reference through every function.
static TEXT_COLOR: AtomicU32 = AtomicU32::new(0x00DCE2F0);
/// Frame colour, same packing. Set from the theme; the panels read it through
/// [`border`].
static BORDER_COLOR: AtomicU32 = AtomicU32::new(0x00636568);

fn pack(color: Color) -> Option<u32> {
    match color {
        Color::Rgb(r, g, b) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        _ => None,
    }
}

pub fn set_text_color(color: Color) {
    if let Some(p) = pack(color) {
        TEXT_COLOR.store(p, Ordering::Relaxed);
    }
}

pub fn set_border_color(color: Color) {
    if let Some(p) = pack(color) {
        BORDER_COLOR.store(p, Ordering::Relaxed);
    }
}

/// The colour ordinary interface text is drawn in.
pub fn text() -> Color {
    let packed = TEXT_COLOR.load(Ordering::Relaxed);
    Color::Rgb((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

/// Whether the user set a desktop background (colour or image).
///
/// When they did, panels stop painting their own opaque fill — otherwise the
/// wallpaper would only ever be visible in the gaps between them, which is not
/// a wallpaper.
static HAS_DESKTOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_has_desktop(on: bool) {
    HAS_DESKTOP.store(on, Ordering::Relaxed);
}

/// The fill for panel and modal bodies: the solid panel colour normally, and
/// **no background at all** once a desktop background is in play.
///
/// "No background" has to mean *not setting one*. [`Color::Reset`] is not
/// transparent — it is SGR 49, the terminal's own default background, which
/// paints straight over the wallpaper. A `Style` with no `bg` leaves whatever
/// the buffer already holds, which is where the picture is.
pub fn panel_style() -> Style {
    if HAS_DESKTOP.load(Ordering::Relaxed) {
        Style::default()
    } else {
        Style::default().bg(PANEL_BG)
    }
}

/// Same, for the app-level fill behind the body.
pub fn app_style() -> Style {
    if HAS_DESKTOP.load(Ordering::Relaxed) {
        Style::default()
    } else {
        Style::default().bg(APP_BG)
    }
}

/// Panel and modal borders. Comes from the theme, so a scheme can give the
/// frame its own colour instead of a dimmed copy of the text.
pub fn border() -> Color {
    let packed = BORDER_COLOR.load(Ordering::Relaxed);
    Color::Rgb((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

// ─── Core palette ──────────────────────────────────────────────────────────

pub const APP_BG:    Color = Color::Rgb(13, 17, 23);
pub const PANEL_BG:  Color = Color::Rgb(22, 27, 34);
pub const BACKDROP:  Color = Color::Rgb(8, 10, 14);
pub const BORDER:    Color = Color::Rgb(48, 54, 61);
pub const BORDER_LT: Color = Color::Rgb(58, 64, 72);
pub const SHADOW:    Color = Color::Rgb(10, 12, 16);
pub const HEADER:    Color = Color::Rgb(240, 136, 62);
pub const ACCENT:    Color = Color::Rgb(31, 111, 235);

pub const OK:        Color = Color::Rgb(56, 200, 100);
pub const ERR:       Color = Color::Rgb(220, 80, 80);
pub const WARN:      Color = Color::Yellow;
pub const DIM:       Color = Color::Rgb(80, 90, 110);
pub const HINT:      Color = Color::Rgb(120, 130, 150);

pub const STATUS_BG: Color = Color::Rgb(18, 22, 28);
pub const STATUS_FG: Color = Color::Rgb(180, 190, 210);

pub const FX_ON:     Color = Color::Rgb(56, 200, 100);
pub const FX_OFF:    Color = Color::Rgb(90, 95, 105);
pub const FX_KNOB:   Color = Color::Rgb(100, 160, 220);

// Splash logo gradient (gold/silver metallic)
pub const SPLASH_GRADIENT: [Color; 12] = [
    Color::Rgb(160, 160, 180),
    Color::Rgb(190, 190, 210),
    Color::Rgb(215, 210, 190),
    Color::Rgb(235, 225, 175),
    Color::Rgb(250, 240, 155),
    Color::Rgb(235, 225, 175),
    Color::Rgb(215, 210, 190),
    Color::Rgb(190, 190, 210),
    Color::Rgb(160, 160, 180),
    Color::Rgb(140, 140, 165),
    Color::Rgb(160, 160, 180),
    Color::Rgb(190, 190, 210),
];

// Spinner chars (braille)
pub const SPINNER: [char; 8] = ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];
pub const SPINNER_DOTS: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
