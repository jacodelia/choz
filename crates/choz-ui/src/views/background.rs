//! What sits behind the whole UI: a flat colour, or an image.
//!
//! Images go through `ratatui-image` in **halfblocks** mode: every cell becomes
//! `▀` with the top half in the foreground colour and the bottom half in the
//! background, so a picture gets **twice the vertical resolution** of one
//! averaged colour per cell.
//!
//! Halfblocks and not kitty/sixel on purpose. The graphics protocols draw
//! outside the cell model, on a layer the terminal composites on top — fine for
//! a picture in a box (that is what the About logo uses), useless for a
//! *background*, because the UI drawn afterwards would not cover it and the text
//! would be unreadable. Halfblocks writes into the cell buffer, so everything
//! else keeps painting over it normally.
//!
//! Whatever is drawn on top must leave `bg` alone: a `Style` with no background,
//! never `Color::Reset` — see [`crate::views::theme::panel_style`].

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::{Image, Resize};

use crate::settings::{Background, ImageFit};

/// Cell size in pixels, used to decide how many image pixels fit in a cell.
///
/// Querying the terminal (`Picker::from_query_stdio`) is not an option here:
/// choz has already taken the terminal over by the time anything is drawn, and
/// the query would corrupt the display. 8×16 is the common ratio, and it only
/// affects how the source is scaled — halfblocks always renders two pixels per
/// cell vertically regardless.
const FONT_SIZE: (u16, u16) = (8, 16);

/// The last decoded source image, kept whole.
///
/// Decoding a photo is by far the slowest step (a JPEG is tens of
/// milliseconds), and it does not depend on the size, the fit or the theme —
/// so changing any of those must not pay for it again. That is what made the
/// FIT toggle feel sticky.
static SOURCE: std::sync::Mutex<Option<(std::path::PathBuf, std::sync::Arc<image::DynamicImage>)>> =
    std::sync::Mutex::new(None);

/// Decode `path`, reusing the last decode when it is the same file.
pub fn decode_cached(path: &std::path::Path) -> Option<std::sync::Arc<image::DynamicImage>> {
    let mut g = SOURCE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, img)) = g.as_ref() {
        if p == path {
            return Some(std::sync::Arc::clone(img));
        }
    }
    let img = std::sync::Arc::new(image::open(path).ok()?);
    *g = Some((path.to_path_buf(), std::sync::Arc::clone(&img)));
    Some(img)
}

/// Average colour of the image under each terminal cell.
///
/// This is what lets a panel look translucent over the wallpaper: it blends its
/// own colour with the picture *at that cell* instead of covering it. Building
/// it is cheap and happens once per image, so the opacity slider changes
/// nothing about the image itself — which is what keeps it instant.
pub fn cell_colors(img: &image::RgbaImage, cols: u16, rows: u16) -> Vec<(u8, u8, u8)> {
    let mut out = Vec::with_capacity(cols as usize * rows as usize);
    let (cw, ch) = (
        (img.width() / cols.max(1) as u32).max(1),
        (img.height() / rows.max(1) as u32).max(1),
    );
    for row in 0..rows as u32 {
        for col in 0..cols as u32 {
            let (x0, y0) = (col * cw, row * ch);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..(y0 + ch).min(img.height()) {
                for x in x0..(x0 + cw).min(img.width()) {
                    let p = img.get_pixel(x, y).0;
                    r += p[0] as u32;
                    g += p[1] as u32;
                    b += p[2] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push(((r / n) as u8, (g / n) as u8, (b / n) as u8));
        }
    }
    out
}

/// The per-cell colours for the whole screen, repeating the tile as the
/// renderer does. This is what the panels blend against.
pub fn backdrop_cells(w: &Wallpaper, area: Rect) -> Vec<(u8, u8, u8)> {
    let (tw, th) = (w.tile.width.max(1), w.tile.height.max(1));
    let mut out = Vec::with_capacity(area.width as usize * area.height as usize);
    for y in 0..area.height {
        for x in 0..area.width {
            let idx = (y % th) as usize * tw as usize + (x % tw) as usize;
            out.push(w.cells.get(idx).copied().unwrap_or((0, 0, 0)));
        }
    }
    out
}

/// A decoded image, already turned into cells for one specific size.
///
/// Decoding and scaling a JPEG on every frame would be absurd, so this is kept
/// until the file, the fit or the terminal size changes.
pub struct Wallpaper {
    key: (std::path::PathBuf, ImageFit, u16, u16),
    protocol: ratatui_image::protocol::Protocol,
    /// Size of one copy, in cells. Equal to the whole area when stretching.
    tile: Rect,
    /// One colour per cell of the tile, for the panels to blend against.
    cells: Vec<(u8, u8, u8)>,
}

/// Build (or reuse) the protocol for `path` at `area`'s size.
///
/// `None` if the image cannot be read — a wallpaper that has been moved or
/// deleted must not stop choz from drawing.
fn load(
    cache: &mut Option<Wallpaper>,
    path: &std::path::Path,
    fit: ImageFit,
    area: Rect,
) -> Option<()> {
    let key = (path.to_path_buf(), fit, area.width, area.height);
    if cache.as_ref().is_some_and(|w| w.key == key) {
        return Some(());
    }
    if area.width == 0 || area.height == 0 {
        return None;
    }

    let img = decode_cached(path)?;

    // One copy covers the whole screen when stretching; a third of the width
    // when tiling, keeping the source's aspect ratio.
    let tile = match fit {
        ImageFit::Stretch => Rect { x: 0, y: 0, width: area.width, height: area.height },
        ImageFit::Tile => {
            let w = (area.width / 3).max(8);
            let px_w = w as u32 * FONT_SIZE.0 as u32;
            let px_h = px_w * img.height().max(1) / img.width().max(1);
            let h = ((px_h / FONT_SIZE.1 as u32).max(4) as u16).min(area.height.max(1));
            Rect { x: 0, y: 0, width: w, height: h }
        }
    };

    // Scaled here rather than through `Resize`, for two reasons: `Fit`/`Scale`
    // keep the aspect ratio and would letterbox a wallpaper (uncovered strips on
    // a background read as a bug), and `Crop` refuses to enlarge an image that
    // is smaller than the area — which is most of them, since a 740×423 photo is
    // smaller than a 150×40 terminal at 8×16 per cell.
    //
    // Lanczos3 rather than the default Nearest: this runs once per resize, and
    // point-sampling a photo down to cell size is exactly where it shows.
    let px = (
        tile.width as u32 * FONT_SIZE.0 as u32,
        tile.height as u32 * FONT_SIZE.1 as u32,
    );
    let img = img.resize_exact(px.0.max(1), px.1.max(1), image::imageops::FilterType::Lanczos3);
    let cells = cell_colors(&img.to_rgba8(), tile.width, tile.height);

    let mut picker = Picker::from_fontsize(FONT_SIZE);
    // Never a graphics protocol: see the module docs.
    picker.set_protocol_type(ProtocolType::Halfblocks);

    // The image already matches the area exactly, so this is a no-op resize.
    let protocol = picker.new_protocol(img, tile, Resize::Fit(None)).ok()?;

    *cache = Some(Wallpaper { key, protocol, tile, cells });
    Some(())
}

/// Paint `bg` over `area` in the frame buffer, before anything else is drawn.
pub fn render(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    bg: &Background,
    cache: &mut Option<Wallpaper>,
) {
    match bg {
        Background::Terminal => {}
        Background::Color((r, g, b)) => {
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(Color::Rgb(*r, *g, *b));
                    }
                }
            }
        }
        Background::Image { path, fit } => {
            if load(cache, path, *fit, area).is_none() {
                return;
            }
            let Some(w) = cache.as_ref() else { return };
            let (tw, th) = (w.tile.width.max(1), w.tile.height.max(1));

            // Stretching draws one copy; tiling walks the screen in steps of one
            // tile. The widget clips itself to the rect it is given, so a tile
            // hanging off the edge is simply cut.
            let mut y = area.y;
            while y < area.bottom() {
                let mut x = area.x;
                while x < area.right() {
                    let slot = Rect {
                        x,
                        y,
                        width: tw.min(area.right() - x),
                        height: th.min(area.bottom() - y),
                    };
                    ratatui::widgets::Widget::render(Image::new(&w.protocol), slot, buf);
                    x = x.saturating_add(tw);
                }
                y = y.saturating_add(th);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect { x: 0, y: 0, width: w, height: h }
    }

    fn sample() -> Option<&'static std::path::Path> {
        let p = std::path::Path::new("../../assets/wallpaper.png");
        if !p.exists() {
            eprintln!("no assets/wallpaper.png; skipping");
            return None;
        }
        Some(p)
    }

    #[test]
    fn a_flat_colour_reaches_every_cell() {
        let a = area(4, 3);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        render(&mut buf, a, &Background::Color((10, 20, 30)), &mut cache);
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(buf[(x, y)].bg, Color::Rgb(10, 20, 30), "cell {x},{y}");
            }
        }
    }

    /// The default must not touch the buffer at all, so the terminal's own
    /// background (and any transparency the user set up) survives.
    #[test]
    fn the_terminal_default_paints_nothing() {
        let a = area(3, 2);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        render(&mut buf, a, &Background::Terminal, &mut cache);
        assert!((0..2).all(|y| (0..3).all(|x| buf[(x, y)].bg == Color::Reset)));
    }

    /// A wallpaper that was moved or deleted must not stop choz from drawing.
    #[test]
    fn a_missing_image_is_survivable() {
        let a = area(4, 2);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        render(
            &mut buf,
            a,
            &Background::Image { path: "/nope/missing.png".into(), fit: ImageFit::Stretch },
            &mut cache,
        );
        assert!(cache.is_none(), "nothing cached for a file that cannot be read");
        assert_eq!(buf[(0, 0)].bg, Color::Reset, "and the buffer is left alone");
    }

    /// Halfblocks put two pixels in every cell: the top half in `fg` behind a
    /// `▀` glyph, the bottom half in `bg`. That is the resolution win over one
    /// averaged colour per cell, so it is asserted rather than assumed.
    #[test]
    fn an_image_fills_the_area_with_two_pixels_per_cell() {
        let Some(path) = sample() else { return };
        let a = area(20, 10);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        render(&mut buf, a, &Background::Image { path: path.into(), fit: ImageFit::Stretch }, &mut cache);

        let w = cache.as_ref().expect("decoded");
        assert_eq!((w.tile.width, w.tile.height), (20, 10), "stretch covers the area");

        let painted = (0..10)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].bg != Color::Reset)
            .count();
        assert!(painted > 150, "most cells got a colour, got {painted}/200");

        // Cells carrying `▀` with *different* fg and bg are two pixels in one
        // cell — the thing a single averaged colour per cell can never produce,
        // and the whole reason for going through ratatui-image.
        let two_tone = (0..10)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = &buf[(x, y)];
                c.symbol() == "\u{2580}" && c.fg != c.bg
            })
            .count();
        assert!(two_tone > 0, "no cell carries two colours; resolution was lost");
    }

    #[test]
    fn the_decoded_image_is_reused_until_something_changes() {
        let Some(path) = sample() else { return };
        let a = area(20, 10);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        let bg = Background::Image { path: path.into(), fit: ImageFit::Stretch };

        render(&mut buf, a, &bg, &mut cache);
        let key = cache.as_ref().unwrap().key.clone();
        render(&mut buf, a, &bg, &mut cache);
        assert_eq!(cache.as_ref().unwrap().key, key, "same key, no re-decode");

        // A different size invalidates it.
        let bigger = area(30, 12);
        let mut buf2 = ratatui::buffer::Buffer::empty(bigger);
        render(&mut buf2, bigger, &bg, &mut cache);
        assert_ne!(cache.as_ref().unwrap().key, key, "resizing rebuilds it");
    }

    #[test]
    fn tiling_repeats_a_smaller_copy() {
        let Some(path) = sample() else { return };
        let a = area(60, 20);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;
        render(&mut buf, a, &Background::Image { path: path.into(), fit: ImageFit::Tile }, &mut cache);

        let w = cache.as_ref().expect("decoded");
        assert!(w.tile.width < a.width, "a tile is smaller than the screen: {}", w.tile.width);
        assert!(w.tile.height > 0);
        // The far side of the screen is painted too, which only happens if the
        // tile was repeated rather than drawn once.
        assert!(
            (0..20).any(|y| buf[(a.width - 1, y)].bg != Color::Reset),
            "the right-hand edge is covered"
        );
    }

    /// Not an assertion — a measurement, printed with `--nocapture`, of how many
    /// distinct colours a full-screen wallpaper produces.
    #[test]
    #[ignore]
    fn measure_resolution() {
        for name in ["wallpaper.png", "wallpaper2.jpg", "wallpaper.jpg"] {
            let path = std::path::PathBuf::from("../../assets").join(name);
            if !path.exists() {
                continue;
            }
            let a = area(150, 40);
            let mut buf = ratatui::buffer::Buffer::empty(a);
            let mut cache = None;
            render(&mut buf, a, &Background::Image { path, fit: ImageFit::Stretch }, &mut cache);
            let mut colours = std::collections::HashSet::new();
            let (mut painted, mut two_tone) = (0, 0);
            for y in 0..a.height {
                for x in 0..a.width {
                    let c = &buf[(x, y)];
                    if c.bg != Color::Reset {
                        painted += 1;
                        colours.insert(format!("{:?}", c.bg));
                    }
                    if c.fg != Color::Reset {
                        colours.insert(format!("{:?}", c.fg));
                    }
                    // Two different colours in one cell = the extra vertical
                    // resolution halfblocks buys. One colour per cell can never
                    // produce this.
                    if c.symbol() == "\u{2580}" && c.fg != c.bg {
                        two_tone += 1;
                    }
                }
            }
            println!(
                "{name:16} pintadas {painted}/6000  celdas de 2 tonos {two_tone}  colores {}",
                colours.len()
            );
        }
    }

    /// The one that matters: after the background is painted, a panel drawn on
    /// top must not erase it.
    ///
    /// `Color::Reset` looks like "transparent" and is not — it is the terminal's
    /// own default background, and it painted straight over the wallpaper. The
    /// only thing that leaves the buffer alone is not setting `bg` at all.
    #[test]
    fn a_panel_drawn_on_top_does_not_erase_the_background() {
        use ratatui::widgets::{Block, Borders, Widget};

        // `has_desktop` is process-wide: without the shared lock another
        // rendering test can flip it back mid-render.
        let _g = crate::views::theme::ui_guard();

        let a = area(10, 4);
        let mut buf = ratatui::buffer::Buffer::empty(a);
        let mut cache = None;

        crate::views::theme::set_has_desktop(true);
        render(&mut buf, a, &Background::Color((90, 40, 120)), &mut cache);
        Block::default()
            .borders(Borders::ALL)
            .style(crate::views::theme::panel_style())
            .render(a, &mut buf);
        crate::views::theme::set_has_desktop(false);

        assert_eq!(
            buf[(5, 2)].bg,
            Color::Rgb(90, 40, 120),
            "the panel body still shows the desktop"
        );
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(90, 40, 120), "and so does the border cell");

        // Without a desktop the panel goes back to painting its own fill.
        let mut buf = ratatui::buffer::Buffer::empty(a);
        Block::default()
            .borders(Borders::ALL)
            .style(crate::views::theme::panel_style())
            .render(a, &mut buf);
        assert_eq!(buf[(5, 2)].bg, crate::views::theme::PANEL_BG, "solid when there is no desktop");
    }
}
