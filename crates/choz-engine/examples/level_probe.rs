//! How loud a slot's source really is, in dBFS, for one note and for a chord.
//!
//! The report is "an SF2 is quieter than a plugin": every source in the rack is
//! summed at the same mixer gain, so whatever difference there is comes from
//! inside the instrument. The same note (C4, velocity 100), the same window,
//! measured on each source given on the command line.
//!
//! ```sh
//! cargo run --release -p choz-engine --example level_probe -- \
//!     /usr/share/sounds/sf2/FluidR3_GM.sf2 \
//!     "clap:/usr/lib/clap/Surge XT.clap"
//! ```
//! A plugin argument is `<format>:<path>[:<id>]` — `clap`, `lv2`, `vst2`,
//! `vst3`, `dssi`, `sfz`; an LV2 wants its URI as the id.
use choz_engine::sources::AudioSource;

/// Sample rate and block, from the environment so a profile can be measured
/// the way it is actually run: `SR=96000 BLOCK=128 level_probe …`.
fn env(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn sr() -> u32 {
    env("SR", 48_000)
}

fn block() -> usize {
    env("BLOCK", 256) as usize
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// Peak and loudest 50 ms RMS of `secs` of rendering, in dBFS.
fn measure(src: &mut dyn AudioSource, secs: f32) -> (f32, f32) {
    let (sr, block) = (sr(), block());
    let mut buf = vec![0.0f32; block * 2];
    let blocks = (sr as f32 * secs / block as f32) as usize;
    let window = (sr as usize / 20 / block).max(1); // ~50 ms in blocks
    let (mut peak, mut best_rms) = (0.0f32, 0.0f32);
    let (mut acc, mut n) = (0.0f64, 0usize);
    for i in 0..blocks {
        buf.fill(0.0);
        src.render(&mut buf, sr);
        for s in &buf {
            peak = peak.max(s.abs());
            acc += (*s as f64) * (*s as f64);
        }
        n += buf.len();
        if (i + 1) % window == 0 {
            best_rms = best_rms.max((acc / n as f64).sqrt() as f32);
            acc = 0.0;
            n = 0;
        }
    }
    (db(peak), db(best_rms))
}

fn open(arg: &str) -> Option<(String, Box<dyn AudioSource>)> {
    use choz_engine::PluginFormat;
    if arg.ends_with(".sf2") {
        let p = std::path::Path::new(arg);
        let s = choz_engine::sources::Sf2Synth::load(p, 0, 0, sr()).ok()?;
        return Some((format!("SF2 {arg} (bank 0 preset 0)"), Box::new(s)));
    }
    let (fmt, rest) = arg.split_once(':')?;
    let format = match fmt {
        "clap" => PluginFormat::Clap,
        "lv2" => PluginFormat::Lv2,
        "dssi" => PluginFormat::Dssi,
        "vst2" => PluginFormat::Vst2,
        "vst3" => PluginFormat::Vst3,
        "sfz" => PluginFormat::Sfz,
        other => {
            eprintln!("unknown format {other}");
            return None;
        }
    };
    // `<format>:<path>@<id>` — the id is optional and may itself contain
    // colons (an LV2 URI), so `@` separates it rather than a second colon.
    let (path, id) = rest.split_once('@').unwrap_or((rest, ""));
    let p = std::path::Path::new(path);
    let src = if format == PluginFormat::Clap {
        choz_plugin_clap::host::ClapInstrument::build(p, id, sr(), block() as u32)
            .map(|i| Box::new(i) as Box<dyn AudioSource>)?
    } else {
        choz_engine::engine::build_instrument(format, p, id, sr(), block() as u32).ok()?
    };
    Some((format!("{} {path} {id}", fmt.to_uppercase()), src))
}

fn main() {
    if choz_engine::worker_main() {
        return;
    }
    println!("{} Hz / {} frames", sr(), block());
    println!(
        "{:<44} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "source", "note pk", "note rms", "chord pk", "chord rms", "pedal pk"
    );
    for arg in std::env::args().skip(1) {
        let Some((name, mut src)) = open(&arg) else {
            eprintln!("cannot open {arg}");
            continue;
        };
        // Half a second of silence first: a plugin may need a block or two
        // before it answers a note at all.
        let mut warm = vec![0.0f32; block() * 2];
        for _ in 0..8 {
            src.render(&mut warm, sr());
        }
        src.note_on(60, 100);
        let one = measure(&mut *src, 2.0);
        let mut settle = vec![0.0f32; block() * 2];
        let mut quiet = |src: &mut dyn AudioSource| {
            src.all_notes_off();
            for _ in 0..(sr() as usize / block()) {
                src.render(&mut settle, sr());
            }
        };
        quiet(&mut *src);
        for n in [60, 64, 67, 72] {
            src.note_on(n, 100);
        }
        let chord = measure(&mut *src, 2.0);
        quiet(&mut *src);
        // The worst a player can ask for: both hands under the sustain pedal,
        // nothing ever released. This is the column that says whether a source
        // still fits under full scale at the slot's own fader.
        src.control_change(64, 127);
        for n in 48..69u8 {
            src.note_on(n, 110);
            src.note_off(n);
        }
        let pedal = measure(&mut *src, 2.0);
        src.control_change(64, 0);
        println!(
            "{:<44} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
            name.chars().take(44).collect::<String>(),
            one.0,
            one.1,
            chord.0,
            chord.1,
            pedal.0
        );
    }
}
