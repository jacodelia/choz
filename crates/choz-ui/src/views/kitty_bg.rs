//! The wallpaper at the terminal's real pixel resolution, under the text.
//!
//! The halfblocks path in [`super::background`] is limited by the cell grid: two
//! pixels per cell vertically, one horizontally. A photo drawn that way is
//! visibly blocky, and no amount of better scaling fixes it — the grid is the
//! ceiling.
//!
//! kitty's graphics protocol has no such ceiling, and one detail makes it usable
//! as a *background* rather than as a picture in a box: **`z` may be negative**,
//! and an image placed at `z < 0` is composited **below the text**. So choz
//! transmits the wallpaper once, scaled to the window's pixel size, places it
//! below the cell backgrounds (see [`Z_UNDER_CELL_BACKGROUNDS`]) covering the
//! whole grid, and then draws the TUI over it as usual.
//!
//! Two conditions this depends on, both already true:
//! - Panels must not paint their own background, or they cover the image. That
//!   is what [`super::theme::panel_style`] does once a desktop is configured.
//! - Cells left at the terminal's default background let the image through;
//!   cells with an explicit background colour hide it. Deliberate highlights
//!   (the selected row, buttons) therefore stay opaque, which is what you want.
//!
//! `ratatui-image` is not used here: its kitty backend places images through
//! Unicode placeholders written *into the cell buffer*, so any text drawn over
//! those cells erases the image. This module speaks the protocol directly to get
//! the negative z-index.

use std::io::Write;

use ratatui::layout::Rect;

use crate::settings::{Background, ImageFit};

/// Image id choz uses for its wallpaper. Fixed, so a new placement can delete
/// the previous one instead of stacking copies in the terminal's memory.
const IMAGE_ID: u32 = 0x63_68_7A; // "chz"
/// Image id of the panel wash. A second, translucent image over the first.
const MASK_ID: u32 = IMAGE_ID + 1;

/// Where the wallpaper sits in kitty's stacking order.
///
/// Any negative `z` puts an image below the text, but the threshold that
/// matters is **-1073741824**: below it, the image is drawn *under the cell
/// backgrounds* too. That is the one choz wants. At `z=-1` the picture would
/// cover every panel and every highlight, since those are cell backgrounds —
/// the selected row would lose its colour. Below the threshold, the image shows
/// exactly where nothing painted a background, which is what
/// [`super::theme::panel_style`] arranges.
const Z_UNDER_CELL_BACKGROUNDS: i64 = -2_000_000_000;

/// Pixels per cell in the wash mask.
///
/// The mask is flat rectangles, so it does not need the picture's resolution —
/// but it is scaled up by the terminal, and at one pixel per cell the
/// interpolation smears a panel's edge across a whole cell. Four keeps the
/// edges tight and still leaves the mask two orders of magnitude smaller than
/// the photo, which is what makes the opacity slider instant.
const MASK_SCALE: u32 = 4;

/// The panel wash sits just above the picture and still under the cell
/// backgrounds, so kitty composites the two and the text draws over both.
const Z_WASH: i64 = Z_UNDER_CELL_BACKGROUNDS + 1;

/// Fallback cell size when the terminal does not report one.
const FALLBACK_CELL: (u16, u16) = (8, 16);

/// What a placement was made for. Re-emitting costs a full image transfer, so
/// it only happens when one of these changes.
#[derive(PartialEq, Clone)]
pub struct Placement {
    path: std::path::PathBuf,
    fit: ImageFit,
    cols: u16,
    rows: u16,
    px: (u16, u16),
}

/// Whether this terminal takes the kitty graphics protocol.
///
/// Detected from the environment rather than by querying: choz is already in
/// raw mode with the alternate screen up by the time anything is drawn, and a
/// query response would land in the input stream. `CHOZ_KITTY_BG=0` forces the
/// halfblocks path back.
pub fn available() -> bool {
    if std::env::var("CHOZ_KITTY_BG").is_ok_and(|v| v == "0") {
        return false;
    }
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM").is_ok_and(|t| t.contains("kitty"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|t| t == "ghostty" || t == "WezTerm")
}

/// The terminal's window size in pixels, and the size of one cell.
fn cell_size(area: Rect) -> (u16, u16) {
    let Ok(ws) = crossterm::terminal::window_size() else {
        return FALLBACK_CELL;
    };
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return FALLBACK_CELL;
    }
    // `window_size` reports the whole window; the grid choz draws into is
    // `area`, which is normally the same thing.
    let _ = area;
    ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1))
}

/// Put `bg` on screen as a real image if this terminal can do it.
///
/// Returns `true` when the image is (or already was) placed, in which case the
/// caller must **not** paint cell backgrounds — the picture is behind them.
/// Returns `false` for anything it cannot handle, so the halfblocks path takes
/// over unchanged.
pub fn sync(
    out: &mut impl Write,
    bg: &Background,
    area: Rect,
    state: &mut Option<Placement>,
    cells: &mut Option<Vec<(u8, u8, u8)>>,
) -> bool {
    if !available() || area.width == 0 || area.height == 0 {
        return false;
    }
    let Background::Image { path, fit } = bg else {
        // A colour background (or none) is drawn by the normal path; make sure
        // no stale image is left underneath it.
        if state.take().is_some() {
            *cells = None;
            let _ = clear(out);
        }
        return false;
    };

    let (cw, ch) = cell_size(area);
    let want = Placement {
        path: path.clone(),
        fit: *fit,
        cols: area.width,
        rows: area.height,
        px: (cw, ch),
    };
    if state.as_ref() == Some(&want) {
        return true; // already on screen; the placement survives redraws
    }

    let px_w = area.width as u32 * cw as u32;
    let px_h = area.height as u32 * ch as u32;
    let Some(image) = render_pixels(path, *fit, px_w, px_h) else {
        // Unreadable file: fall back to halfblocks, which reports it the same
        // way it always has.
        *state = None;
        return false;
    };

    // The panels need to know what is behind them: under this protocol the
    // cell buffer holds nothing at all.
    *cells = Some(super::background::cell_colors(
        &image,
        area.width,
        area.height,
    ));

    let _ = clear(out);
    if place(out, image.as_raw(), px_w, px_h, area).is_err() {
        *state = None;
        *cells = None;
        return false;
    }
    *state = Some(want);
    true
}

/// Remove choz's wallpaper from the terminal. Called when the background is
/// turned off and on the way out, so the image does not outlive the app.
pub fn clear(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\")?;
    out.flush()
}

/// Decode `path` and produce exactly `px_w`×`px_h` RGBA pixels, stretched or
/// tiled. Full window resolution — this is the whole point of the module.
fn render_pixels(
    path: &std::path::Path,
    fit: ImageFit,
    px_w: u32,
    px_h: u32,
) -> Option<image::RgbaImage> {
    // Shared with the halfblocks path: the decode is the expensive part and
    // it does not depend on the size or the fit.
    let img = super::background::decode_cached(path)?;
    let canvas = match fit {
        ImageFit::Stretch => image::imageops::resize(
            &img.to_rgba8(),
            px_w,
            px_h,
            image::imageops::FilterType::Lanczos3,
        ),
        ImageFit::Tile => {
            let tile = img.to_rgba8();
            let (tw, th) = (tile.width().max(1), tile.height().max(1));
            let mut canvas = image::RgbaImage::new(px_w, px_h);
            let mut y = 0;
            while y < px_h {
                let mut x = 0;
                while x < px_w {
                    image::imageops::replace(&mut canvas, &tile, x as i64, y as i64);
                    x += tw;
                }
                y += th;
            }
            canvas
        }
    };
    Some(canvas)
}

/// Transmit the pixels and place them under the text.
///
/// `a=T` transmits and displays in one go; `f=32` is RGBA; `z` puts the image
/// under the cell backgrounds; `C=1` keeps the cursor where it was, so
/// ratatui's own positioning is untouched. `c`/`r` scale it over the whole grid.
fn place(
    out: &mut impl Write,
    rgba: &[u8],
    px_w: u32,
    px_h: u32,
    area: Rect,
) -> std::io::Result<()> {
    place_image(
        out,
        IMAGE_ID,
        rgba,
        px_w,
        px_h,
        area,
        Z_UNDER_CELL_BACKGROUNDS,
    )
}

/// Transmit `rgba` and place it over the whole grid at `z`.
fn place_image(
    out: &mut impl Write,
    id: u32,
    rgba: &[u8],
    px_w: u32,
    px_h: u32,
    area: Rect,
    z: i64,
) -> std::io::Result<()> {
    // The image is placed at the cursor, so it has to start at the top left.
    write!(out, "\x1b[H")?;

    // base64-simd is already in the tree (ratatui-image pulls it in for the
    // same protocol), so no new dependency for this.
    let payload = base64_simd::STANDARD.encode_to_string(rgba);
    // 4096 bytes of base64 per escape is the chunk size the protocol specifies.
    let mut chunks = payload.as_bytes().chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        if first {
            write!(
                out,
                "\x1b_Ga=T,f=32,t=d,q=2,i={id},s={px_w},v={px_h},c={},r={},z={z},C=1,m={more};",
                area.width, area.height,
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gq=2,m={more};")?;
        }
        out.write_all(chunk)?;
        write!(out, "\x1b\\")?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stretching fills the requested pixel buffer exactly — the size the
    /// placement escape claims it is sending.
    #[test]
    fn stretching_produces_exactly_the_requested_pixels() {
        let dir = std::env::temp_dir().join(format!("choz_kbg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        image::RgbaImage::from_pixel(4, 3, image::Rgba([9, 8, 7, 255]))
            .save(&path)
            .unwrap();

        let px = render_pixels(&path, ImageFit::Stretch, 40, 32).unwrap();
        assert_eq!(px.as_raw().len(), 40 * 32 * 4);
        // Tiling covers the whole canvas too, even when the tile does not divide
        // the area.
        let px = render_pixels(&path, ImageFit::Tile, 41, 33).unwrap();
        assert_eq!(px.as_raw().len(), 41 * 33 * 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Everything that depends on the environment, in **one** test: these
    /// variables are process-wide and the harness runs test functions in
    /// parallel, so two of them toggling `CHOZ_KITTY_BG` race.
    ///
    /// (1) The escape hatch wins over detection. (2) The wash is a separate,
    /// translucent image: the picture is transmitted once and never touched
    /// again, which is what keeps its full resolution — only the mask moves
    /// when the opacity or the layout does.
    #[test]
    fn the_graphics_path_is_switchable_and_the_wash_is_its_own_image() {
        unsafe { std::env::set_var("CHOZ_KITTY_BG", "0") };
        assert!(
            !available(),
            "the escape hatch beats the terminal detection"
        );
        unsafe { std::env::remove_var("CHOZ_KITTY_BG") };

        let area = Rect::new(0, 0, 10, 4);
        let rects = [(Rect::new(0, 0, 4, 2), 0.5)];
        let mut state = None;
        let mut out: Vec<u8> = Vec::new();

        // Detection is by environment; force the path on for the test.
        unsafe { std::env::set_var("KITTY_WINDOW_ID", "1") };
        sync_mask(&mut out, area, (10, 20, 30), &rects, &mut state);
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(
            text.contains(&format!("i={MASK_ID}")),
            "the mask has its own image id"
        );
        assert!(
            text.contains(&format!("z={Z_WASH}")),
            "over the picture, under the text"
        );
        assert!(
            !text.contains(&format!("i={IMAGE_ID},")),
            "the picture itself must not be re-sent for a wash"
        );

        // Same layout and opacity: nothing goes out at all.
        out.clear();
        sync_mask(&mut out, area, (10, 20, 30), &rects, &mut state);
        assert!(out.is_empty(), "an unchanged wash costs nothing");

        // A different opacity does re-send it.
        out.clear();
        sync_mask(
            &mut out,
            area,
            (10, 20, 30),
            &[(Rect::new(0, 0, 4, 2), 0.9)],
            &mut state,
        );
        assert!(!out.is_empty());
        unsafe { std::env::remove_var("KITTY_WINDOW_ID") };
    }

    /// A wallpaper that has been deleted must not stop choz from drawing.
    #[test]
    fn a_missing_file_is_not_fatal() {
        assert!(render_pixels(
            std::path::Path::new("/nope/none.png"),
            ImageFit::Stretch,
            8,
            8
        )
        .is_none());
    }
}

// ─── The panel wash ─────────────────────────────────────────────────────────

/// What the wash was last drawn for. Cheap to compare, so the mask is only
/// re-sent when the layout, the colour or the opacity actually moved.
#[derive(PartialEq, Clone, Default)]
pub struct MaskState {
    rects: Vec<(Rect, u8)>,
    color: (u8, u8, u8),
    cols: u16,
    rows: u16,
}

/// Wash the panel rectangles with the theme colour **without touching the
/// picture**.
///
/// Painting cell backgrounds would work, and it is what the halfblocks path
/// does — but here it would cover the image the terminal is drawing, and one
/// colour per cell is exactly the blockiness this path exists to avoid. So the
/// wash is a second image: the theme colour with an alpha channel, one pixel
/// per cell, placed above the photo and below the text. kitty scales it to the
/// grid and composites it, so the photo keeps every pixel it had.
///
/// One pixel per cell is deliberate: the mask is flat rectangles, so a few
/// kilobytes redraw instantly when the opacity slider moves.
pub fn sync_mask(
    out: &mut impl Write,
    area: Rect,
    color: (u8, u8, u8),
    rects: &[(Rect, f32)],
    state: &mut Option<MaskState>,
) {
    if !available() || area.width == 0 || area.height == 0 {
        return;
    }
    let want = MaskState {
        rects: rects
            .iter()
            .map(|(r, a)| (*r, (a.clamp(0.0, 1.0) * 255.0) as u8))
            .collect(),
        color,
        cols: area.width,
        rows: area.height,
    };
    if state.as_ref() == Some(&want) {
        return;
    }
    if want.rects.is_empty() {
        let _ = clear_mask(out);
        *state = Some(want);
        return;
    }

    let (mw, mh) = (
        area.width as u32 * MASK_SCALE,
        area.height as u32 * MASK_SCALE,
    );
    let mut mask = image::RgbaImage::new(mw, mh);
    for (rect, alpha) in &want.rects {
        let px = image::Rgba([color.0, color.1, color.2, *alpha]);
        for y in
            rect.top() as u32 * MASK_SCALE..(rect.bottom().min(area.height) as u32 * MASK_SCALE)
        {
            for x in
                rect.left() as u32 * MASK_SCALE..(rect.right().min(area.width) as u32 * MASK_SCALE)
            {
                mask.put_pixel(x, y, px);
            }
        }
    }

    let _ = clear_mask(out);
    if place_image(out, MASK_ID, mask.as_raw(), mw, mh, area, Z_WASH).is_err() {
        *state = None;
        return;
    }
    *state = Some(want);
}

/// Remove the wash (leaving the picture alone).
pub fn clear_mask(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={MASK_ID},q=2\x1b\\")?;
    out.flush()
}
