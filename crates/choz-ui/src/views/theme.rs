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
    match panel_fill() {
        // A desktop with no picture in it — a flat colour, or the terminal's
        // own. There is nothing to see *through*, so the translucency is done
        // once, here: the panel colour is the desktop's with the tint mixed in
        // at the configured strength.
        Some(c) => Style::default().bg(c),
        None if HAS_DESKTOP.load(Ordering::Relaxed) => Style::default(),
        None => Style::default().bg(PANEL_BG),
    }
}

/// The colour panels paint when there is no picture behind them, already
/// blended. `u32::MAX` means "unset" — a real colour never is, because the top
/// byte is always zero.
static PANEL_FILL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// Publish the blended panel colour, or `None` to go back to the flat one.
pub fn set_panel_fill(rgb: Option<(u8, u8, u8)>) {
    let packed = match rgb {
        Some((r, g, b)) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        None => u32::MAX,
    };
    PANEL_FILL.store(packed, Ordering::Relaxed);
}

pub fn panel_fill() -> Option<Color> {
    match PANEL_FILL.load(Ordering::Relaxed) {
        u32::MAX => None,
        p => Some(Color::Rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)),
    }
}

/// The channels of an RGB colour. Only `Color::Rgb` has any — the palette is
/// all `Rgb`, and anything else has no numbers to blend.
pub fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// `base` with `tint` mixed in at `alpha`. The one place a translucent colour
/// can exist in a terminal: a cell background has no alpha, so "semi
/// transparent" has to be resolved to a real colour before it is painted.
pub fn blend(base: (u8, u8, u8), tint: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    (
        mix(base.0, tint.0),
        mix(base.1, tint.1),
        mix(base.2, tint.2),
    )
}

/// Same, for the app-level fill behind the body.
pub fn app_style() -> Style {
    if HAS_DESKTOP.load(Ordering::Relaxed) {
        Style::default()
    } else {
        Style::default().bg(APP_BG)
    }
}

/// The desktop as seen one colour per cell, and how the panels wash over it.
///
/// A terminal cell background is **opaque**: "a translucent panel over the
/// wallpaper" cannot be expressed as a colour on top. What it *can* be is a
/// blend — panel colour mixed with what the picture shows at that very cell —
/// and that is what this table is for. It is filled when the wallpaper is
/// built (both drawing paths compute it) and read once per cell by [`wash`].
static DESKTOP_CELLS: std::sync::RwLock<Option<Backdrop>> = std::sync::RwLock::new(None);

pub struct Backdrop {
    pub cols: u16,
    pub rows: u16,
    /// Average colour of the image under each cell, row-major.
    pub cells: Vec<(u8, u8, u8)>,
    /// The theme colour panels are washed with, and how strongly (0..1).
    pub tint: ((u8, u8, u8), f32),
    /// True when the terminal is drawing the picture itself, at real pixel
    /// resolution, below the cells.
    ///
    /// Then the wash must **not** touch cell backgrounds: an opaque cell hides
    /// the image, and one colour per cell is exactly the blockiness the
    /// graphics protocol was there to avoid. The panels are washed by a second,
    /// translucent image instead — see `views::kitty_bg::sync_mask`.
    pub graphics: bool,
}

impl Backdrop {
    fn at(&self, x: u16, y: u16) -> Option<(u8, u8, u8)> {
        if x >= self.cols || y >= self.rows {
            return None;
        }
        self.cells
            .get(y as usize * self.cols as usize + x as usize)
            .copied()
    }
}

/// Publish the per-cell picture (and the panel wash) for this screen size.
/// `None` clears it — no image, no blend, panels go back to their flat colour.
pub fn set_backdrop(backdrop: Option<Backdrop>) {
    if let Ok(mut g) = DESKTOP_CELLS.write() {
        *g = backdrop;
    }
}

/// Wash `area` with the theme's panel colour over whatever the picture shows
/// there, so the text drawn next reads without hiding the wallpaper.
///
/// No-op when there is no picture: the panels then paint their own opaque
/// colour through [`panel_style`], exactly as they always did.
pub fn wash(buf: &mut ratatui::buffer::Buffer, area: ratatui::layout::Rect) {
    wash_with(buf, area, 1.0)
}

/// [`wash`] with the configured opacity scaled by `strength`.
fn wash_with(buf: &mut ratatui::buffer::Buffer, area: ratatui::layout::Rect, strength: f32) {
    let Ok(g) = DESKTOP_CELLS.read() else { return };
    let Some(bd) = g.as_ref() else { return };
    // The terminal owns the picture: washing cells here would cover it.
    if bd.graphics {
        return;
    }
    let ((tr, tg, tb), alpha) = bd.tint;
    let a = (alpha * strength).clamp(0.0, 1.0);
    let mix = |base: u8, tint: u8| (base as f32 * (1.0 - a) + tint as f32 * a) as u8;
    let blend = |c: Color| match c {
        Color::Rgb(r, g, b) => Color::Rgb(mix(r, tr), mix(g, tg), mix(b, tb)),
        other => other,
    };
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some((br, bg_, bb)) = bd.at(x, y) else {
                continue;
            };
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            // Halfblocks put the top pixel in `fg` behind a `▀` and the bottom
            // one in `bg`. Washing only the background would throw away half of
            // the image's vertical resolution before anything was even drawn on
            // top, so both are blended and the glyph is left alone.
            let fg = blend(cell.fg);
            cell.set_fg(fg);
            cell.set_bg(Color::Rgb(mix(br, tr), mix(bg_, tg), mix(bb, tb)));
        }
    }
}

/// A softer wash, for the menu and status bars: they are one row each and a
/// full-strength panel colour there reads as a stripe across the picture.
pub fn wash_weak(buf: &mut ratatui::buffer::Buffer, area: ratatui::layout::Rect) {
    wash_with(buf, area, WEAK_WASH)
}

/// How much of the configured opacity the one-row bars get.
pub const WEAK_WASH: f32 = 0.6;

/// The fill for anything drawn *over* the body — modals, menus, the About box.
///
/// Always opaque, desktop or not: these are preceded by a `Clear`, which resets
/// the cells it covers, and a wallpaper showing through a modal is a hole, not
/// a feature. Panels get [`panel_style`]; overlays get this.
pub fn overlay_style() -> Style {
    Style::default().bg(PANEL_BG)
}

/// Panel and modal borders. Comes from the theme, so a scheme can give the
/// frame its own colour instead of a dimmed copy of the text.
pub fn border() -> Color {
    let packed = BORDER_COLOR.load(Ordering::Relaxed);
    Color::Rgb((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

// ─── Core palette ──────────────────────────────────────────────────────────

pub const APP_BG: Color = Color::Rgb(13, 17, 23);
pub const PANEL_BG: Color = Color::Rgb(22, 27, 34);
pub const BACKDROP: Color = Color::Rgb(8, 10, 14);
pub const BORDER: Color = Color::Rgb(48, 54, 61);
pub const BORDER_LT: Color = Color::Rgb(58, 64, 72);
pub const SHADOW: Color = Color::Rgb(10, 12, 16);
pub const HEADER: Color = Color::Rgb(240, 136, 62);
pub const ACCENT: Color = Color::Rgb(31, 111, 235);

pub const OK: Color = Color::Rgb(56, 200, 100);
pub const ERR: Color = Color::Rgb(220, 80, 80);
pub const WARN: Color = Color::Yellow;
pub const DIM: Color = Color::Rgb(80, 90, 110);
pub const HINT: Color = Color::Rgb(120, 130, 150);

pub const STATUS_BG: Color = Color::Rgb(18, 22, 28);
pub const STATUS_FG: Color = Color::Rgb(180, 190, 210);

pub const FX_ON: Color = Color::Rgb(56, 200, 100);
pub const FX_OFF: Color = Color::Rgb(90, 95, 105);
pub const FX_KNOB: Color = Color::Rgb(100, 160, 220);

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

// Whether this thread already holds `ui_guard`'s lock.
#[cfg(test)]
thread_local! {
    static UI_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The lock every test that touches the process-wide look has to hold.
///
/// Language, text colour, the desktop flag and the backdrop are all globals, so
/// two rendering tests running in parallel can undo each other's setup. It
/// lives here rather than in the UI's test module because the panels' own tests
/// need it too.
///
/// **Reentrant, deliberately.** A `std::sync::Mutex` is not, and the helpers
/// that render a panel take this lock themselves — so a test that takes it and
/// then renders deadlocks against itself, taking every other test waiting on
/// the same lock down with it. It looked like a slow suite rather than a
/// failure (every thread parked in `futex_do_wait` at 0 % CPU), and it cost two
/// sessions to find. Twice. A thread-local "already held" flag makes taking it
/// twice on one thread free, while two threads still take turns.
#[cfg(test)]
pub fn ui_guard() -> UiGuard {
    static UI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if UI_HELD.with(|h| h.get()) {
        return UiGuard {
            _inner: None,
            outermost: false,
        };
    }
    let guard = UI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    UI_HELD.with(|h| h.set(true));
    UiGuard {
        _inner: Some(guard),
        outermost: true,
    }
}

/// What [`ui_guard`] hands back. Holds the real guard only for the outermost
/// take on this thread.
#[cfg(test)]
pub struct UiGuard {
    _inner: Option<std::sync::MutexGuard<'static, ()>>,
    outermost: bool,
}

#[cfg(test)]
impl Drop for UiGuard {
    fn drop(&mut self) {
        if self.outermost {
            UI_HELD.with(|h| h.set(false));
        }
    }
}
