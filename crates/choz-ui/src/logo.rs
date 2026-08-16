//! Builds the choz logo as a ratatui-image [`Protocol`] rendered in the About
//! dialog. The image is generated in memory (no asset file) so it works out of
//! the box; Halfblocks protocol renders in any terminal (no sixel/kitty needed).

use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;

/// Build the logo protocol. Returns `None` if image encoding fails (About then
/// falls back to text).
pub fn build_logo() -> Option<Protocol> {
    let img = generate();
    let mut picker = Picker::from_fontsize((8, 16));
    // Force halfblocks: portable to every terminal, no capability query needed.
    picker.set_protocol_type(ProtocolType::Halfblocks);
    picker
        .new_protocol(img, Rect::new(0, 0, 44, 12), Resize::Fit(None))
        .ok()
}

/// A diagonal HSV-rainbow banner — the same gradient spirit as the splash.
fn generate() -> DynamicImage {
    const W: u32 = 352;
    const H: u32 = 96;
    let mut buf = RgbaImage::new(W, H);
    for y in 0..H {
        for x in 0..W {
            // Hue sweeps diagonally; brightness dips at top/bottom for a band look.
            let hue = ((x + y) as f32 / (W + H) as f32) * 360.0;
            let vshade = 1.0 - ((y as f32 / H as f32) - 0.5).abs(); // 0.5..1.0
            let (r, g, b) = hsv_to_rgb(hue, 0.85, 0.55 + 0.45 * vshade);
            buf.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
    DynamicImage::ImageRgba8(buf)
}

/// Also used by the keyboard visualizer to give each MIDI channel a hue.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0)); // red
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0)); // green
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255)); // blue
    }

    #[test]
    fn generates_expected_size() {
        assert_eq!(generate().to_rgba8().dimensions(), (352, 96));
    }

    #[test]
    fn build_logo_produces_a_protocol() {
        // Halfblocks encoding must succeed on the generated image.
        assert!(build_logo().is_some());
    }
}
