//! Hue helper: each MIDI channel and each rack tab gets its own colour from it.
//! The About box used to show a generated rainbow banner built here; it shows
//! the splash logo now, so only the colour maths is left.

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
}
