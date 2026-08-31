//! Manual check: what does one knob's worth of CC traffic cost the UI thread?
//! `cargo run -p choz-engine --release --example cc_storm`

use choz_engine::fx_chain::{build_chain_from_specs, FxSpec};

fn main() {
    // A typical rack: the FX a MIDI-learned knob would be driving.
    for kind in ["delay", "reverb", "grandelay", "compressor"] {
        let mk = || FxSpec {
            gate: None,
            kind: kind.to_string(),
            enabled: true,
            wet: 0.5,
            params: vec![0.5; 8],
            plugin: None,
            loops: Vec::new(),
            loop_frames: 0,
        };
        let n = 200;
        let t = std::time::Instant::now();
        let mut sink = Vec::new();
        for _ in 0..n {
            sink.push(build_chain_from_specs(&[mk()], 48_000, 64));
        }
        let each = t.elapsed() / n;
        drop(sink);
        println!(
            "{kind:<8} rebuild = {each:?} each  → {:.0} rebuilds/s sustainable",
            1.0 / each.as_secs_f64()
        );
    }
}
