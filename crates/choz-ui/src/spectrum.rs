//! The spectrum analyser: an FFT of what is coming out, on the UI thread.
//!
//! **Never in the callback.** The audio thread writes raw samples into
//! [`choz_engine::meter::Meter`]'s ring and forgets about them; this reads that
//! ring whenever the interface redraws and does the arithmetic there, where an
//! occasional millisecond costs a frame and not an xrun.
//!
//! # What comes out
//!
//! One magnitude in dB per FFT bin, plus a **peak hold** per bin that falls
//! slowly. The bins are linear in frequency — the log scale belongs to the
//! drawing, which knows how many columns it has; holding the peaks per bin
//! rather than per column means resizing the panel does not throw them away.
//!
//! # Why the FFT is written here
//!
//! It is forty lines, it runs a few thousand times a second at most, and the
//! alternative is a dependency whose SIMD planning is worth exactly nothing at
//! this size.

use choz_engine::meter::{meter, SPECTRUM_POINTS};

/// Bins the analysis produces: the useful half of the transform.
pub const BINS: usize = SPECTRUM_POINTS / 2;

/// Where the display bottoms out. Below this a bin is silence as far as a
/// terminal is concerned.
pub const FLOOR_DB: f32 = -78.0;

/// How fast a held peak falls, in dB per redraw. At ~20 redraws a second a
/// peak takes about a second and a half to walk down 40 dB — long enough to
/// read, short enough not to be a smear of everything that ever played.
const PEAK_FALL_DB: f32 = 1.4;

pub struct Spectrum {
    /// Magnitudes of the last analysis, in dB, `FLOOR_DB` at the bottom.
    mags: Vec<f32>,
    /// The highest each bin has been lately, falling.
    peaks: Vec<f32>,
    /// Scratch, so a redraw allocates nothing.
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    sample_rate: f32,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    pub fn new() -> Self {
        // Hann: the cheapest window that stops a tone between two bins from
        // smearing across all of them. Without one, every steady note shows up
        // as a plateau and the picture says nothing.
        let window = (0..SPECTRUM_POINTS)
            .map(|i| {
                let t = i as f32 / (SPECTRUM_POINTS - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        Self {
            mags: vec![FLOOR_DB; BINS],
            peaks: vec![FLOOR_DB; BINS],
            re: vec![0.0; SPECTRUM_POINTS],
            im: vec![0.0; SPECTRUM_POINTS],
            window,
            sample_rate: 48_000.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr.max(8_000.0);
    }

    /// The bin a frequency falls in.
    pub fn hz_bin(&self, hz: f32) -> usize {
        ((hz * SPECTRUM_POINTS as f32 / self.sample_rate).round() as usize).min(BINS - 1)
    }

    /// Read the meter's window and analyse it. Called once per redraw.
    pub fn update(&mut self) {
        let mut window = [0.0f32; SPECTRUM_POINTS];
        meter().spectrum_window(&mut window);
        self.analyse(&window);
    }

    /// The analysis itself, against a window handed in — which is what makes
    /// it testable without an audio device.
    pub fn analyse(&mut self, samples: &[f32]) {
        for i in 0..SPECTRUM_POINTS {
            self.re[i] = samples.get(i).copied().unwrap_or(0.0) * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);

        // A Hann window throws away half the energy, and the transform of a
        // real signal splits each tone between its two mirrored bins: the
        // scaling below is what makes a full-scale sine read as 0 dB rather
        // than as "some number that goes up when it gets louder".
        let scale = 4.0 / SPECTRUM_POINTS as f32;
        for i in 0..BINS {
            let mag = (self.re[i] * self.re[i] + self.im[i] * self.im[i]).sqrt() * scale;
            let db = if mag > 1e-9 {
                20.0 * mag.log10()
            } else {
                FLOOR_DB
            };
            let db = db.max(FLOOR_DB);
            self.mags[i] = db;
            self.peaks[i] = if db >= self.peaks[i] {
                db
            } else {
                (self.peaks[i] - PEAK_FALL_DB).max(db)
            };
        }
    }

    /// Fold the bins into `width` columns spread **logarithmically** over
    /// 20 Hz–20 kHz, as `(level, peak)` in 0..1 of the display range.
    ///
    /// Each column takes the loudest bin it covers, not the average: a spectrum
    /// analyser that averages hides the very peak it exists to show, and in the
    /// treble one column covers hundreds of bins.
    pub fn columns(&self, width: usize) -> Vec<(f32, f32)> {
        let mut out = Vec::with_capacity(width);
        if width == 0 {
            return out;
        }
        let norm = |db: f32| ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        for col in 0..width {
            let lo_hz = col_hz(col as f32, width);
            let hi_hz = col_hz(col as f32 + 1.0, width);
            let lo = self.hz_bin(lo_hz).max(1);
            // At the bass end a column is narrower than a bin; take that one
            // bin rather than an empty range.
            let hi = self.hz_bin(hi_hz).max(lo + 1).min(BINS);
            let (mut m, mut p) = (FLOOR_DB, FLOOR_DB);
            for i in lo..hi {
                m = m.max(self.mags[i]);
                p = p.max(self.peaks[i]);
            }
            out.push((norm(m), norm(p)));
        }
        out
    }

    /// Where a frequency marker goes, as a column, or `None` if it is off the
    /// scale this display covers.
    pub fn marker_col(&self, hz: f32, width: usize) -> Option<usize> {
        if width == 0 || !(LOW_HZ..=HIGH_HZ).contains(&hz) {
            return None;
        }
        let t = (hz / LOW_HZ).log(HIGH_HZ / LOW_HZ);
        Some(((t * width as f32).round() as usize).min(width - 1))
    }
}

/// The ends of the drawn scale. Below 20 Hz and above 20 kHz there is nothing
/// to see that is not either the DC bin or the anti-aliasing filter.
pub const LOW_HZ: f32 = 20.0;
pub const HIGH_HZ: f32 = 20_000.0;

fn col_hz(col: f32, width: usize) -> f32 {
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(col / width as f32)
}

/// In-place iterative radix-2 FFT. `re` and `im` must be the same power-of-two
/// length.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    // Bit reversal.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    // Butterflies, doubling the stage length each time.
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, sr: f32, amp: f32) -> Vec<f32> {
        (0..SPECTRUM_POINTS)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin() * amp)
            .collect()
    }

    /// The whole claim: a 1 kHz tone shows up at 1 kHz, at its own level, and
    /// nowhere else.
    #[test]
    fn a_tone_lands_in_its_own_bin_at_its_own_level() {
        let mut s = Spectrum::new();
        s.set_sample_rate(48_000.0);
        s.analyse(&tone(1000.0, 48_000.0, 1.0));

        let bin = s.hz_bin(1000.0);
        let db = s.mags[bin];
        assert!(
            (db - 0.0).abs() < 1.0,
            "a full-scale sine should read ~0 dB, got {db}"
        );
        // Two bins away in either direction, the Hann window has already
        // dropped it a long way.
        assert!(
            s.mags[bin - 4] < db - 30.0,
            "leaking down: {}",
            s.mags[bin - 4]
        );
        assert!(
            s.mags[bin + 4] < db - 30.0,
            "leaking up: {}",
            s.mags[bin + 4]
        );
        // And somewhere else entirely there is nothing.
        assert!(s.mags[s.hz_bin(5000.0)] < FLOOR_DB + 20.0);
    }

    /// Half the amplitude is 6 dB down, which is the only way to know the
    /// scaling is a scaling and not a shape.
    #[test]
    fn halving_the_signal_takes_six_decibels_off() {
        let mut s = Spectrum::new();
        s.set_sample_rate(48_000.0);
        s.analyse(&tone(1000.0, 48_000.0, 1.0));
        let loud = s.mags[s.hz_bin(1000.0)];
        s.analyse(&tone(1000.0, 48_000.0, 0.5));
        let quiet = s.mags[s.hz_bin(1000.0)];
        assert!(
            (loud - quiet - 6.02).abs() < 0.5,
            "expected 6 dB, got {}",
            loud - quiet
        );
    }

    /// Silence reads as the floor, not as whatever was there before.
    #[test]
    fn silence_is_the_floor() {
        let mut s = Spectrum::new();
        s.analyse(&tone(1000.0, 48_000.0, 1.0));
        s.analyse(&vec![0.0; SPECTRUM_POINTS]);
        assert!(s.mags.iter().all(|d| *d <= FLOOR_DB + 1e-3));
        // …but the peak hold is still on its way down, which is its job.
        assert!(s.peaks[s.hz_bin(1000.0)] > FLOOR_DB + 10.0);
    }

    /// The peak hold falls, and it gets all the way back to the floor.
    #[test]
    fn the_peak_hold_falls_back_to_the_floor() {
        let mut s = Spectrum::new();
        s.analyse(&tone(1000.0, 48_000.0, 1.0));
        let bin = s.hz_bin(1000.0);
        let held = s.peaks[bin];
        assert!(held > -3.0);
        let silence = vec![0.0; SPECTRUM_POINTS];
        for _ in 0..10 {
            s.analyse(&silence);
        }
        assert!(s.peaks[bin] < held - 10.0, "the peak did not fall");
        for _ in 0..200 {
            s.analyse(&silence);
        }
        assert!(
            (s.peaks[bin] - FLOOR_DB).abs() < 1e-3,
            "it stopped short of the floor at {}",
            s.peaks[bin]
        );
    }

    /// The columns are logarithmic: an octave is the same width wherever it is.
    #[test]
    fn the_columns_are_spread_by_octaves() {
        let s = Spectrum::new();
        let width = 60;
        let a = s.marker_col(100.0, width).unwrap();
        let b = s.marker_col(200.0, width).unwrap();
        let c = s.marker_col(4000.0, width).unwrap();
        let d = s.marker_col(8000.0, width).unwrap();
        let low_octave = b - a;
        let high_octave = d - c;
        assert!(
            low_octave.abs_diff(high_octave) <= 1,
            "octaves should be the same width: {low_octave} vs {high_octave}"
        );
        assert!(s.marker_col(5.0, width).is_none(), "below the scale");
        assert!(s.marker_col(30_000.0, width).is_none(), "above it");
    }

    /// A tone shows up in the column its frequency belongs to, and the columns
    /// either side of it are quiet.
    #[test]
    fn a_tone_shows_up_in_the_right_column() {
        let mut s = Spectrum::new();
        s.set_sample_rate(48_000.0);
        s.analyse(&tone(1000.0, 48_000.0, 1.0));
        let width = 60;
        let cols = s.columns(width);
        assert_eq!(cols.len(), width);
        let at = s.marker_col(1000.0, width).unwrap();
        let loudest = cols
            .iter()
            .enumerate()
            .max_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap())
            .unwrap()
            .0;
        assert!(
            loudest.abs_diff(at) <= 1,
            "1 kHz should be at column {at}, the loudest is {loudest}"
        );
        // Every column is inside the display range.
        assert!(cols
            .iter()
            .all(|(m, p)| (0.0..=1.0).contains(m) && (0.0..=1.0).contains(p) && p >= m));
    }

    /// A width of zero is a panel too narrow to draw in, not a panic.
    #[test]
    fn a_zero_width_panel_asks_for_nothing() {
        let s = Spectrum::new();
        assert!(s.columns(0).is_empty());
        assert_eq!(s.marker_col(1000.0, 0), None);
    }

    /// The transform against a known answer: an impulse is flat, and DC is DC.
    #[test]
    fn the_transform_is_a_transform() {
        let mut re = vec![0.0f32; 16];
        let mut im = vec![0.0f32; 16];
        re[0] = 1.0;
        fft(&mut re, &mut im);
        for (r, i) in re.iter().zip(im.iter()) {
            assert!((r - 1.0).abs() < 1e-5 && i.abs() < 1e-5, "impulse → flat");
        }

        let mut re = vec![1.0f32; 16];
        let mut im = vec![0.0f32; 16];
        fft(&mut re, &mut im);
        assert!((re[0] - 16.0).abs() < 1e-4, "DC → one bin of N");
        assert!(re[1..].iter().all(|r| r.abs() < 1e-4));
    }
}
