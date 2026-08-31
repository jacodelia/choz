//! Ten-band graphic EQ, Winamp layout — the one from tanu, made an FX.
//!
//! tanu (`src/audio/eq.rs`) applies ten cascaded RBJ peaking biquads per
//! channel over its player's stream; here the same filters sit in a rack slot's
//! chain, so the bands are choz parameters: **MIDI-learnable and automatable one
//! by one**, like every other knob. The presets are tanu's, which are Winamp's.
//!
//! The band frequencies and the ±12 dB range are Winamp's too, so a preset means
//! the same thing here as it does there.

use choz_ports::{FxParam, FxProcessor};

pub const EQ_BANDS: usize = 10;

/// Winamp classic centre frequencies (Hz).
pub const EQ_FREQS: [f32; EQ_BANDS] = [
    70.0, 180.0, 320.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];

/// Gain range of each band, in dB.
pub const EQ_MAX_DB: f32 = 12.0;

/// The original Winamp EQ presets, as tanu carries them (from
/// github.com/schollz/Winamp-Original-Presets). Winamp stores each band as a
/// 0–64 slider (33 = 0 dB, ±12 dB range), converted to dB with
/// `(value - 33) * 12/32`.
pub const PRESETS: &[(&str, [f32; EQ_BANDS])] = &[
    ("Flat", [0.0; EQ_BANDS]),
    (
        "Classical",
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -4.875, -4.875, -4.875, -6.375],
    ),
    (
        "Club",
        [0.0, 0.0, 1.875, 3.375, 3.375, 3.375, 1.875, 0.0, 0.0, 0.0],
    ),
    (
        "Dance",
        [
            5.625, 4.125, 1.125, -0.375, -0.375, -4.125, -4.875, -4.875, -0.375, -0.375,
        ],
    ),
    (
        "Full Bass",
        [
            5.625, 5.625, 5.625, 3.375, 0.75, -3.0, -5.625, -6.75, -7.125, -7.125,
        ],
    ),
    (
        "Full Bass & Treble",
        [
            4.125, 3.375, 0.0, -4.875, -3.375, 0.75, 4.875, 6.375, 7.125, 7.125,
        ],
    ),
    (
        "Full Treble",
        [
            -6.375, -6.375, -6.375, -3.0, 1.5, 6.375, 9.375, 9.375, 9.375, 10.125,
        ],
    ),
    (
        "Laptop/Headphones",
        [
            2.625, 6.375, 3.0, -2.625, -1.875, 0.75, 2.625, 5.625, 7.5, 8.625,
        ],
    ),
    (
        "Large Hall",
        [
            6.0, 6.0, 3.375, 3.375, 0.0, -3.375, -3.375, -3.375, 0.0, 0.0,
        ],
    ),
    (
        "Live",
        [-3.375, 0.0, 2.25, 3.0, 3.375, 3.375, 2.25, 1.5, 1.5, 1.125],
    ),
    (
        "Party",
        [4.125, 4.125, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.125, 4.125],
    ),
    (
        "Pop",
        [
            -1.5, 2.625, 4.125, 4.5, 3.0, -1.125, -1.875, -1.875, -1.5, -1.5,
        ],
    ),
    (
        "Reggae",
        [0.0, 0.0, -0.75, -4.125, 0.0, 3.75, 3.75, 0.0, 0.0, 0.0],
    ),
    (
        "Rock",
        [
            4.5, 2.625, -3.75, -5.25, -2.625, 2.25, 5.25, 6.375, 6.375, 6.375,
        ],
    ),
    (
        "Ska",
        [
            -1.875, -3.375, -3.0, -0.75, 2.25, 3.375, 5.25, 5.625, 6.375, 5.625,
        ],
    ),
    (
        "Soft",
        [
            2.625, 0.75, -1.125, -1.875, -1.125, 2.25, 4.875, 5.625, 6.375, 7.125,
        ],
    ),
    (
        "Soft Rock",
        [
            2.25, 2.25, 1.125, -0.75, -3.0, -3.75, -2.625, -0.75, 1.5, 5.25,
        ],
    ),
    (
        "Techno",
        [4.5, 3.375, 0.0, -3.75, -3.375, 0.0, 4.5, 5.625, 5.625, 5.25],
    ),
];

/// Which preset a knob position (0..1) picks. 0 is "Flat", which is also what a
/// knob left alone means.
pub fn preset_index(norm: f32) -> usize {
    (norm.clamp(0.0, 1.0) * (PRESETS.len() - 1) as f32).round() as usize
}

/// A knob position (0..1) for `db`, and back. The middle of the knob is flat,
/// which is what a graphic EQ's slider row looks like at rest.
pub fn db_to_norm(db: f32) -> f32 {
    (db / (2.0 * EQ_MAX_DB) + 0.5).clamp(0.0, 1.0)
}

pub fn norm_to_db(norm: f32) -> f32 {
    (norm.clamp(0.0, 1.0) - 0.5) * 2.0 * EQ_MAX_DB
}

/// One RBJ peaking biquad, transposed direct form II — tanu's, per channel.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Peaking EQ coefficients (RBJ cookbook). The filter's state is left alone:
    /// a band moved while audio runs must not click.
    fn set_peaking(&mut self, freq: f32, gain_db: f32, q: f32, fs: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * (freq / fs).min(0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        self.b0 = (1.0 + alpha * a) / a0;
        self.b1 = (-2.0 * cos) / a0;
        self.b2 = (1.0 - alpha * a) / a0;
        self.a1 = (-2.0 * cos) / a0;
        self.a2 = (1.0 - alpha / a) / a0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The FX: ten bands and a preamp, per stereo channel.
pub struct GraphicEq {
    gains_db: [f32; EQ_BANDS],
    preamp_db: f32,
    /// Which of [`PRESETS`] was last loaded. Kept only so the picker can be
    /// read back — a host that cannot ask where a knob is cannot save it.
    preset: usize,
    wet: f32,
    filters: [[Biquad; EQ_BANDS]; 2],
    /// Sample rate the coefficients were computed at; a change recomputes them.
    fs: f32,
    dirty: bool,
}

impl Default for GraphicEq {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphicEq {
    pub fn new() -> Self {
        Self {
            gains_db: [0.0; EQ_BANDS],
            preamp_db: 0.0,
            preset: 0,
            wet: 1.0,
            filters: [[Biquad::identity(); EQ_BANDS]; 2],
            fs: 0.0,
            dirty: true,
        }
    }

    /// Load one of [`PRESETS`] by index. Out of range leaves it alone.
    ///
    /// **Re-picking the preset that is already loaded does nothing**, and that
    /// is what makes the EQ safe to drive by parameter index. A host replays
    /// every parameter in ascending order when it activates a plugin — see
    /// `plugin_activate` in `choz-plugin-clap-export` — so the preset at index
    /// 11 arrives *after* the ten band gains it would overwrite: without this
    /// guard, a session whose bands were shaped by hand snapped back to the
    /// preset's table on every activate.
    pub fn set_preset(&mut self, index: usize) {
        if index == self.preset {
            return;
        }
        if let Some((_, gains)) = PRESETS.get(index) {
            self.gains_db = *gains;
            self.preset = index;
            self.dirty = true;
        }
    }

    pub fn set_band_db(&mut self, band: usize, db: f32) {
        if band < EQ_BANDS {
            self.gains_db[band] = db.clamp(-EQ_MAX_DB, EQ_MAX_DB);
            self.dirty = true;
        }
    }

    pub fn set_preamp_db(&mut self, db: f32) {
        self.preamp_db = db.clamp(-EQ_MAX_DB, EQ_MAX_DB);
        self.dirty = true;
    }

    pub fn gains_db(&self) -> [f32; EQ_BANDS] {
        self.gains_db
    }

    fn recompute(&mut self, fs: f32) {
        for chan in self.filters.iter_mut() {
            for (i, bq) in chan.iter_mut().enumerate() {
                // The state (`z1`, `z2`) is deliberately kept: recomputing
                // coefficients mid-stream is a knob turn, not a reset.
                bq.set_peaking(EQ_FREQS[i], self.gains_db[i], 1.0, fs);
            }
        }
        self.fs = fs;
        self.dirty = false;
    }
}

impl FxProcessor for GraphicEq {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let fs = sample_rate.max(1) as f32;
        if self.dirty || (self.fs - fs).abs() > 0.5 {
            self.recompute(fs);
        }
        let preamp = 10f32.powf(self.preamp_db / 20.0);
        for frame in buf.as_chunks_mut::<2>().0 {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let dry = *sample;
                let mut x = dry * preamp;
                for bq in self.filters[ch].iter_mut() {
                    x = bq.process(x);
                }
                *sample = dry + (x - dry) * self.wet;
            }
        }
    }

    fn reset(&mut self) {
        for chan in self.filters.iter_mut() {
            for bq in chan.iter_mut() {
                bq.z1 = 0.0;
                bq.z2 = 0.0;
            }
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "GRAPHIC EQ"
    }

    fn params(&self) -> Vec<FxParam> {
        let mut out: Vec<FxParam> = EQ_FREQS
            .iter()
            .enumerate()
            .map(|(i, _)| {
                FxParam::new(
                    BAND_NAMES[i],
                    db_to_norm(self.gains_db[i]),
                    -EQ_MAX_DB,
                    EQ_MAX_DB,
                    "dB",
                )
            })
            .collect();
        out.push(FxParam::new(
            "Preamp",
            db_to_norm(self.preamp_db),
            -EQ_MAX_DB,
            EQ_MAX_DB,
            "dB",
        ));
        // The picker and the mix, which the interface draws and a host was
        // never told about: this list is what the exported CLAP plugin
        // publishes, so a knob missing here is a knob a DAW cannot move,
        // automate or save.
        out.push(FxParam::new(
            "Preset",
            self.preset as f32 / (PRESETS.len() - 1) as f32,
            0.0,
            (PRESETS.len() - 1) as f32,
            "",
        ));
        out.push(FxParam::new("Wet", self.wet, 0.0, 1.0, ""));
        out
    }

    /// Bands 0..9, then the preamp, then the preset picker — the same order the
    /// UI draws them, so a CC learned on "1 kHz" stays on 1 kHz.
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0..=9 => self.set_band_db(index, norm_to_db(value)),
            10 => self.set_preamp_db(norm_to_db(value)),
            // The preset is a knob like the rest, so it has to work like one:
            // without this, picking "Rock" only took effect the next time the
            // chain was rebuilt (a project reload), which reads as a dead knob.
            11 => self.set_preset(preset_index(value)),
            12 => self.wet = value.clamp(0.0, 1.0),
            _ => {}
        }
    }
}

/// Band labels, short enough for a 13-column knob cell.
pub const BAND_NAMES: [&str; EQ_BANDS] = [
    "70", "180", "320", "600", "1k", "3k", "6k", "12k", "14k", "16k",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A band that is boosted has to make its own frequency louder and leave a
    /// far-away one alone — otherwise it is a filter in name only.
    #[test]
    fn a_boosted_band_lifts_its_own_frequency() {
        let sr = 48_000u32;
        let tone = |hz: f32, eq: &mut GraphicEq| -> f32 {
            eq.reset();
            let frames = sr as usize / 4;
            let mut buf = vec![0.0f32; frames * 2];
            for f in 0..frames {
                let s = (2.0 * std::f32::consts::PI * hz * f as f32 / sr as f32).sin();
                buf[f * 2] = s;
                buf[f * 2 + 1] = s;
            }
            eq.process_block(&mut buf, sr);
            // Skip the filter's settling time, then take the peak.
            buf[frames..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };

        let mut flat = GraphicEq::new();
        let flat_1k = tone(1000.0, &mut flat);
        assert!((flat_1k - 1.0).abs() < 0.05, "flat is flat, got {flat_1k}");

        let mut boosted = GraphicEq::new();
        boosted.set_band_db(4, 12.0); // the 1 kHz band
        let loud_1k = tone(1000.0, &mut boosted);
        let far = tone(70.0, &mut boosted);
        assert!(
            loud_1k > flat_1k * 2.0,
            "1 kHz should be lifted: {loud_1k} vs {flat_1k}"
        );
        assert!(far < flat_1k * 1.3, "70 Hz is a different band: {far}");
    }

    /// The presets are the reason to have this rather than four parametric
    /// bands, so they have to arrive intact and mean dB.
    #[test]
    fn the_winamp_presets_come_across_whole() {
        assert_eq!(PRESETS.len(), 18, "Winamp's set, as tanu carries it");
        assert_eq!(PRESETS[0].0, "Flat");
        assert_eq!(PRESETS[0].1, [0.0; EQ_BANDS]);
        let rock = PRESETS
            .iter()
            .find(|(n, _)| *n == "Rock")
            .expect("Rock is one of them");
        assert_eq!(rock.1[0], 4.5, "bass up");
        assert!(rock.1[3] < 0.0, "mids scooped");
        assert!(PRESETS
            .iter()
            .all(|(_, g)| g.iter().all(|db| db.abs() <= EQ_MAX_DB)));

        let mut eq = GraphicEq::new();
        let i = PRESETS.iter().position(|(n, _)| *n == "Rock").unwrap();
        eq.set_preset(i);
        assert_eq!(eq.gains_db(), rock.1);
        eq.set_preset(999);
        assert_eq!(
            eq.gains_db(),
            rock.1,
            "an index out of range changes nothing"
        );
    }

    /// Every band is a parameter, so every band can be learned and automated —
    /// which is the "with MIDI" half of the request.
    #[test]
    fn every_band_is_a_parameter_a_cc_can_reach() {
        let mut eq = GraphicEq::new();
        assert_eq!(
            eq.params().len(),
            EQ_BANDS + 3,
            "ten bands, the preamp, the preset and the mix"
        );
        assert_eq!(eq.params()[4].name, "1k");

        // The middle of the knob is flat, the top is +12 dB.
        eq.set_param(4, 0.5);
        assert!(eq.gains_db()[4].abs() < 1e-6);
        eq.set_param(4, 1.0);
        assert_eq!(eq.gains_db()[4], EQ_MAX_DB);
        eq.set_param(4, 0.0);
        assert_eq!(eq.gains_db()[4], -EQ_MAX_DB);
        assert_eq!(db_to_norm(norm_to_db(0.73)), 0.73);
    }

    /// The preset picker is param 11, and it has to work while audio runs —
    /// not only when the chain is rebuilt.
    #[test]
    fn the_preset_knob_loads_a_preset_live() {
        let mut eq = GraphicEq::new();
        let rock = PRESETS.iter().position(|(n, _)| *n == "Rock").unwrap();
        eq.set_param(11, rock as f32 / (PRESETS.len() - 1) as f32);
        assert_eq!(eq.gains_db(), PRESETS[rock].1);

        // And a band moved afterwards still moves.
        eq.set_param(4, 1.0);
        assert_eq!(eq.gains_db()[4], EQ_MAX_DB);

        assert_eq!(preset_index(0.0), 0, "a knob left alone is Flat");
        assert_eq!(preset_index(1.0), PRESETS.len() - 1);
    }
}
