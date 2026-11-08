//! What `choz Mic` carries: the main pair made safe for a meeting app.
//!
//! Zoom, Meet and Teams record a microphone in **mono** unless told otherwise,
//! and a plain `(L + R) / 2` cancels whatever a widener, a chorus or a
//! ping-pong put in antiphase — measured: a 1 kHz tone at -6 dBFS in antiphase
//! came out of `choz Mic` recorded mono at -180 dBFS. So the fold here is
//! **in quadrature**: the side signal is put 90° from the mid before they are
//! summed, which keeps the energy of both (`|M + jS|² = |M|² + |S|²`) — nothing
//! cancels, a centred source is untouched and a hard-panned one is 3 dB down,
//! flat across the band. The same mono goes out on both channels.
//!
//! After the fold, a limiter at -1 dBFS: the main fader reaches +6 dB and a
//! meeting app turns the float it is handed into 16-bit, where past full scale
//! is a hard clip its AGC then pumps around.
//!
//! The click is **not** here: the copy is taken before the metronome is summed
//! into the main pair, so the student hears the playing, not the count.
//!
//! `CHOZ_MIC_STEREO=1` keeps the stereo image (for Zoom with stereo turned on);
//! the limiter stays either way.

use crate::fx::compressor::Compressor;
use crate::fx::freq_shift::Hilbert;
use crate::fx::FxProcessor;

/// Ceiling of the microphone, in dBFS. A codec downstream needs the headroom.
const CEILING_DB: f32 = -1.0;

pub(crate) struct MicFeed {
    /// Interleaved stereo, filled with the main pair before the click.
    pub(crate) buf: Vec<f32>,
    mid: Hilbert,
    side: Hilbert,
    limiter: Compressor,
    stereo: bool,
}

impl MicFeed {
    pub(crate) fn new(sample_rate: u32, frames: usize) -> Self {
        let mut limiter = Compressor::limiter(sample_rate);
        limiter.threshold_db = CEILING_DB;
        Self {
            buf: vec![0.0; frames * 2],
            mid: Hilbert::new(),
            side: Hilbert::new(),
            limiter,
            stereo: std::env::var_os("CHOZ_MIC_STEREO").is_some_and(|v| v == "1"),
        }
    }

    /// Copy the main pair in, before anything else is summed onto it.
    pub(crate) fn take(&mut self, l: &[f32], r: &[f32], frames: usize) {
        let n = frames.min(l.len()).min(r.len()).min(self.buf.len() / 2);
        for f in 0..n {
            self.buf[f * 2] = l[f];
            self.buf[f * 2 + 1] = r[f];
        }
    }

    /// Main fader, fold and ceiling, in place on [`Self::buf`].
    pub(crate) fn finish(&mut self, gain: [f32; 2], frames: usize, sample_rate: u32) {
        let n = frames.min(self.buf.len() / 2);
        let buf = &mut self.buf[..n * 2];
        for f in buf.as_chunks_mut::<2>().0 {
            let (l, r) = (f[0] * gain[0], f[1] * gain[1]);
            if self.stereo {
                (f[0], f[1]) = (l, r);
                continue;
            }
            let m = self.mid.process((l + r) * 0.5).0;
            let s = self.side.process((l - r) * 0.5).1;
            f[0] = m + s;
            f[1] = m + s;
        }
        self.limiter.process_block(buf, sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const N: usize = 4800;

    /// Peak of the mono a meeting app makes, `(L + R) / 2`, after settling.
    fn meeting_peak(l: &[f32], r: &[f32], gain: f32) -> f32 {
        let mut feed = MicFeed::new(SR, N);
        feed.stereo = false;
        let mut peak = 0f32;
        for pass in 0..4 {
            feed.take(l, r, N);
            feed.finish([gain; 2], N, SR);
            if pass == 3 {
                peak = feed.buf[N..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .fold(0f32, |p, f| p.max(((f[0] + f[1]) * 0.5).abs()));
            }
        }
        peak
    }

    fn tone(phase: f32) -> Vec<f32> {
        // 1 kHz: a whole number of cycles in the block, so passes join up.
        (0..N)
            .map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / SR as f32 + phase).sin())
            .collect()
    }

    #[test]
    fn antiphase_survives_the_mono_a_meeting_makes() {
        let (l, r) = (tone(0.0), tone(std::f32::consts::PI));
        let plain = l
            .iter()
            .zip(&r)
            .fold(0f32, |p, (a, b)| p.max(((a + b) * 0.5).abs()));
        assert!(plain < 1e-3, "the plain fold cancels it: {plain}");
        let peak = meeting_peak(&l, &r, 1.0);
        assert!(
            (peak - 0.5).abs() < 0.03,
            "antiphase reaches the call: {peak}"
        );
    }

    #[test]
    fn a_centred_source_keeps_its_level() {
        let l = tone(0.0);
        let peak = meeting_peak(&l, &l, 1.0);
        assert!((peak - 0.5).abs() < 0.03, "centre: {peak}");
    }

    #[test]
    fn the_main_fader_past_full_scale_stops_at_the_ceiling() {
        let l = tone(0.0);
        // +6 dB on a -6 dBFS tone is full scale, +12 dB (gain 4) is past it.
        let peak = meeting_peak(&l, &l, 4.0);
        let ceiling = 10f32.powf(CEILING_DB / 20.0);
        assert!(peak <= ceiling + 0.01, "{peak} over {ceiling}");
        assert!(peak > ceiling - 0.1, "and not far under it: {peak}");
    }
}
