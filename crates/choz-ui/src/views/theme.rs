//! Shared color palette — adapted from seqterm's theme.
//!
//! All panels, modals, buttons, and overlays use these constants.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use ratatui::style::Color;

/// User-selected text colour (packed 0x00RRGGBB), read by every panel through
/// [`text`]. An atomic keeps it reachable from the draw code without threading
/// a settings reference through every function.
static TEXT_COLOR: AtomicU32 = AtomicU32::new(0x00DCE2F0);

pub fn set_text_color(color: Color) {
    if let Color::Rgb(r, g, b) = color {
        TEXT_COLOR.store(((r as u32) << 16) | ((g as u32) << 8) | b as u32, Ordering::Relaxed);
    }
}

/// The colour ordinary interface text is drawn in.
pub fn text() -> Color {
    let packed = TEXT_COLOR.load(Ordering::Relaxed);
    Color::Rgb((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

/// Panel and modal borders: the text colour, dimmed, so the frame reads as
/// structure rather than competing with the content.
pub fn border() -> Color {
    let packed = TEXT_COLOR.load(Ordering::Relaxed);
    let dim = |c: u32| ((c * 45) / 100) as u8;
    Color::Rgb(dim(packed >> 16 & 0xFF), dim(packed >> 8 & 0xFF), dim(packed & 0xFF))
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
