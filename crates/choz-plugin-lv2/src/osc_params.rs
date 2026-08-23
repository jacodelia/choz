//! The parameters a plugin keeps behind its OSC server, as knobs.
//!
//! ZynAddSubFX publishes sixteen control ports called `Slot 1`…`Slot 16` and
//! nothing else: every real control of the synth — the filter, the envelopes,
//! the oscillator's harmonics — lives in its own tree, reachable only over the
//! OSC server [`crate::osc`] finds. A rack tab holding it had sixteen numbered
//! knobs that name nothing.
//!
//! This is a **short, chosen list**, not the tree: Zyn has thousands of paths,
//! and a panel with thousands of knobs is the same as no panel. What is here is
//! what a player reaches for while playing — level, pan, the amplitude
//! envelope, the filter, the oscillator's shape — with the harmonics of the
//! sound edited in their own view (they are 128 bars, not a knob).
//!
//! The names and ranges are written down rather than queried: `/path-search`
//! does not resolve these concrete paths (it answers for the registry's own
//! prefixes), and a list this short is one that can simply be right.

use choz_ports::PluginParam;

/// What kind of control a path is, which is what decides how it is drawn and
/// how a 0..1 knob position is turned into what is sent.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// A whole number in `min..=max`.
    Int(i32, i32),
    /// On or off, sent as OSC's own `T`/`F`.
    Bool,
    /// A whole number whose positions have names.
    Enum(&'static [&'static str]),
}

/// One knob: where it lives, what it is called, and what it holds.
pub struct OscParam {
    pub path: String,
    pub name: String,
    /// The section it belongs to, which the panel prints as a heading.
    pub group: &'static str,
    pub kind: Kind,
    pub default: i32,
    /// What the value is, when saying so changes how it is drawn: a run of
    /// parameters sharing one of these is a **bank of vertical bars**, which is
    /// the only readable way to show a set of harmonics.
    pub unit: Option<&'static str>,
}

/// A knob with nothing special about it: a number, or a switch.
fn knob(path: &str, name: &str, group: &'static str, kind: Kind, default: i32) -> OscParam {
    OscParam {
        path: path.to_string(),
        name: name.to_string(),
        group,
        kind,
        default,
        unit: None,
    }
}

/// Zyn's own names for the oscillator's base waveform, in port order.
const BASE_FUNCS: &[&str] = &[
    "sine",
    "triangle",
    "pulse",
    "saw",
    "power",
    "gauss",
    "diode",
    "abssine",
    "pulsesine",
    "stretchsine",
    "chirp",
    "absstretchsine",
    "chebyshev",
    "sqr",
    "spike",
    "circle",
    "hypsec",
];

/// And for the analogue filter's type.
const FILTER_TYPES: &[&str] = &[
    "LPF1", "HPF1", "LPF2", "HPF2", "BPF2", "NF2", "PkF2", "LSh2", "HSh2",
];

/// How harmonic magnitudes are scaled — the `Phmagtype` port.
const MAG_TYPES: &[&str] = &["linear", "-40 dB", "-60 dB", "-80 dB", "-100 dB"];

/// How many harmonics of the oscillator are offered as knobs.
///
/// The synth has 128 of each; a panel that shows a dozen cells at a time would
/// need seven pages for them alone, and the whole set is what the HARMONICS
/// view is for. Thirty-two is the part of the spectrum that shapes what the
/// sound is — and it is where its own editor puts the sliders anyone reaches
/// for.
pub const HARMONIC_KNOBS: usize = 32;

/// The unit that makes a run of parameters draw as a bank of vertical bars.
/// It is a tag, not a measurement: what it says is "these belong together and
/// their shape is the point".
const HARMONIC_UNIT: &str = "harmonic";
const PHASE_UNIT: &str = "phase";

/// The list, for a plugin whose URI starts with ZynAddSubFX's.
///
/// Part 0, kit 0, voice 0: one rack tab is one part, and a tab that layers is
/// layered by choz, not inside the synth.
///
/// Built once rather than written out: the harmonics are sixty-four entries
/// that differ only by their number.
pub static ZYN: std::sync::LazyLock<Vec<OscParam>> = std::sync::LazyLock::new(|| {
    let mut out = vec![
        knob("/part0/Pvolume", "VOLUME", "PART", Kind::Int(0, 127), 96),
        knob("/part0/Ppanning", "PAN", "PART", Kind::Int(0, 127), 64),
        knob("/part0/Pvelsns", "VEL SNS", "PART", Kind::Int(0, 127), 64),
        knob("/part0/Ppolymode", "POLY", "PART", Kind::Bool, 1),
        knob("/part0/Plegatomode", "LEGATO", "PART", Kind::Bool, 0),
        knob(
            "/part0/ctl/portamento.portamento",
            "PORTA",
            "PART",
            Kind::Bool,
            0,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/PVolume",
            "VOLUME",
            "ADSYNTH",
            Kind::Int(0, 127),
            90,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/PPanning",
            "PAN",
            "ADSYNTH",
            Kind::Int(0, 127),
            64,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/PDetune",
            "DETUNE",
            "ADSYNTH",
            Kind::Int(0, 16383),
            8192,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/PStereo",
            "STEREO",
            "ADSYNTH",
            Kind::Bool,
            1,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/AmpEnvelope/PA_dt",
            "ATTACK",
            "AMP ENV",
            Kind::Int(0, 127),
            0,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/AmpEnvelope/PD_dt",
            "DECAY",
            "AMP ENV",
            Kind::Int(0, 127),
            40,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/AmpEnvelope/PS_val",
            "SUSTAIN",
            "AMP ENV",
            Kind::Int(0, 127),
            127,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/AmpEnvelope/PR_dt",
            "RELEASE",
            "AMP ENV",
            Kind::Int(0, 127),
            25,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/GlobalFilter/Pfreq",
            "CUTOFF",
            "FILTER",
            Kind::Int(0, 127),
            127,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/GlobalFilter/Pq",
            "RES",
            "FILTER",
            Kind::Int(0, 127),
            40,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/GlobalFilter/Ptype",
            "TYPE",
            "FILTER",
            Kind::Enum(FILTER_TYPES),
            2,
        ),
        knob(
            "/part0/kit0/adpars/GlobalPar/FilterEnvelope/PA_dt",
            "F.ATTACK",
            "FILTER",
            Kind::Int(0, 127),
            40,
        ),
        knob(
            "/part0/kit0/adpars/VoicePar0/OscilSmp/Pcurrentbasefunc",
            "WAVE",
            "OSC",
            Kind::Enum(BASE_FUNCS),
            0,
        ),
        knob(
            "/part0/kit0/adpars/VoicePar0/OscilSmp/Phmagtype",
            "H.SCALE",
            "OSC",
            Kind::Enum(MAG_TYPES),
            0,
        ),
    ];
    // The oscillator's own spectrum, as the two rows of sliders its editor
    // draws: the magnitude of each harmonic, then the phase of each. `64` is
    // silence for both, which is why the bars sit half-way up until they are
    // moved.
    for i in 0..HARMONIC_KNOBS {
        out.push(OscParam {
            path: format!("{ZYN_HARMONICS}{i}"),
            name: format!("H{}", i + 1),
            group: "HARMONICS",
            kind: Kind::Int(0, 127),
            default: if i == 0 { 127 } else { 64 },
            unit: Some(HARMONIC_UNIT),
        });
    }
    for i in 0..HARMONIC_KNOBS {
        out.push(OscParam {
            path: format!("{ZYN_PHASES}{i}"),
            name: format!("P{}", i + 1),
            group: "H.PHASE",
            kind: Kind::Int(0, 127),
            default: 64,
            unit: Some(PHASE_UNIT),
        });
    }
    out
});

/// The list for `plugin_uri`, or nothing — which is every other plugin.
pub fn table_for(plugin_uri: &str) -> Option<&'static [OscParam]> {
    plugin_uri
        .starts_with("http://zynaddsubfx.sourceforge.net")
        .then(|| ZYN.as_slice())
}

impl OscParam {
    fn range(&self) -> (f64, f64) {
        match self.kind {
            Kind::Int(lo, hi) => (lo as f64, hi as f64),
            Kind::Bool => (0.0, 1.0),
            Kind::Enum(names) => (0.0, (names.len().max(1) - 1) as f64),
        }
    }

    /// A 0..1 knob position as the whole number the path holds.
    pub fn plain(&self, norm: f32) -> i32 {
        let (lo, hi) = self.range();
        (lo + (hi - lo) * norm.clamp(0.0, 1.0) as f64).round() as i32
    }
}

/// The list as choz's own parameters, so these knobs are MIDI-learnable and
/// saved with a project like any other plugin's.
pub fn params(table: &'static [OscParam]) -> Vec<PluginParam> {
    table
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (min, max) = p.range();
            PluginParam {
                id: i as u32,
                name: p.name.clone(),
                min,
                max,
                default: p.default as f64,
                steps: match p.kind {
                    Kind::Int(lo, hi) => (hi - lo + 1).clamp(0, 128) as u32,
                    Kind::Bool => 2,
                    Kind::Enum(names) => names.len() as u32,
                },
                unit: p.unit.map(|u| u.to_string()),
                points: match p.kind {
                    Kind::Enum(names) => names
                        .iter()
                        .enumerate()
                        .map(|(v, n)| (v as f64, n.to_string()))
                        .collect(),
                    _ => Vec::new(),
                },
                group: Some(p.group.to_string()),
            }
        })
        .collect()
}

// ─── Sending them ───────────────────────────────────────────────────────────

/// One knob move, on its way from the audio thread to the OSC socket:
/// `(index into the table, 0..1 position)`.
pub type Move = (u16, f32);

/// How many moves can be in flight. A knob is turned by a person or by a CC
/// stream; a hundred is more than either produces between two wake-ups, and
/// dropping the oldest of a burst loses nothing a later one does not correct.
pub const RING: usize = 128;

/// Start the thread that turns knob moves into OSC messages.
///
/// **This is why there is a thread at all**: `set_param` runs on the audio
/// thread, and a UDP send there is a syscall in the callback. The audio side
/// only writes into a lock-free ring; everything that can block happens here.
pub fn spawn_sender(
    client: std::sync::Arc<crate::osc::OscClient>,
    table: &'static [OscParam],
    mut moves: rtrb::Consumer<Move>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let alive = std::sync::Arc::new(AtomicBool::new(true));
    let flag = std::sync::Arc::clone(&alive);
    let spawned = std::thread::Builder::new()
        .name("choz-lv2-osc-params".into())
        .spawn(move || {
            while flag.load(Ordering::Relaxed) {
                let mut idle = true;
                while let Ok((index, norm)) = moves.pop() {
                    idle = false;
                    let Some(p) = table.get(index as usize) else {
                        continue;
                    };
                    let value = p.plain(norm);
                    match p.kind {
                        Kind::Bool => client.send(&p.path, &[crate::osc::Arg::Bool(value != 0)]),
                        _ => client.send(&p.path, &[crate::osc::Arg::Int(value)]),
                    }
                }
                if idle {
                    // Turning a knob is not an emergency; this is well inside
                    // what a hand or a CC stream can tell apart.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        });
    if spawned.is_err() {
        alive.store(false, Ordering::Relaxed);
    }
    alive
}

/// Where one oscillator's harmonics live, and how many there are.
///
/// The names come from the plugin's own metadata: `magnitude#128` and
/// `phase#128`, `0..127` each, **64 meaning silent** — which is why the bars are
/// drawn from the middle and not from the floor. (`Phmag<n>` is the same
/// control under its internal name; it accepts a write and answers no read, so
/// it is not the one used.)
const ZYN_HARMONICS: &str = "/part0/kit0/adpars/VoicePar0/OscilSmp/magnitude";
const ZYN_PHASES: &str = "/part0/kit0/adpars/VoicePar0/OscilSmp/phase";
const ZYN_HARMONIC_COUNT: usize = 128;

/// The plugin's OSC server as choz's own by-path control surface — what the
/// harmonics view edits through.
pub struct OscPaths {
    pub client: std::sync::Arc<crate::osc::OscClient>,
    /// Whether this plugin is one whose harmonics choz knows how to reach.
    pub harmonics: bool,
    /// The knobs this plugin's parameters are, in the order they are reported.
    pub table: &'static [OscParam],
}

impl choz_ports::PluginPaths for OscPaths {
    fn set(&self, path: &str, value: f32) {
        // Whole numbers: every path this reaches holds one. A float where the
        // plugin wants an int is a message it drops on the floor.
        self.client
            .send(path, &[crate::osc::Arg::Int(value.round() as i32)]);
    }

    fn ask(&self, path: &str) {
        self.client.ask(path);
    }

    fn value(&self, path: &str) -> Option<f32> {
        self.client.value(path)
    }

    fn param_paths(&self) -> Vec<String> {
        self.table.iter().map(|p| p.path.clone()).collect()
    }

    fn harmonics(&self) -> Option<choz_ports::HarmonicSet> {
        self.harmonics.then(|| choz_ports::HarmonicSet {
            magnitude: (0..ZYN_HARMONIC_COUNT)
                .map(|i| format!("{ZYN_HARMONICS}{i}"))
                .collect(),
            phase: (0..ZYN_HARMONIC_COUNT)
                .map(|i| format!("{ZYN_PHASES}{i}"))
                .collect(),
            min: 0.0,
            max: 127.0,
            zero: 64.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is a list of knobs, and every one of them has to make sense as
    /// one: a range with room in it, a default inside that range.
    #[test]
    fn every_knob_has_a_range_its_default_sits_in() {
        for p in ZYN.iter() {
            let (lo, hi) = p.range();
            assert!(hi > lo, "{} has no range", p.name);
            assert!(
                (p.default as f64) >= lo && (p.default as f64) <= hi,
                "{}'s default {} is outside {lo}..{hi}",
                p.name,
                p.default
            );
            assert!(p.path.starts_with('/'), "{} is not a path", p.name);
        }
        // The ends of a knob are the ends of the port.
        let vol = &ZYN[0];
        assert_eq!(vol.plain(0.0), 0);
        assert_eq!(vol.plain(1.0), 127);
        assert_eq!(vol.plain(0.5), 64);
    }

    /// A parameter list choz can draw: named, grouped, and enumerations that
    /// carry their names.
    #[test]
    fn the_knobs_come_out_named_and_grouped() {
        let ps = params(ZYN.as_slice());
        assert_eq!(ps.len(), ZYN.len());
        assert!(ps.iter().all(|p| !p.name.is_empty() && p.group.is_some()));
        let wave = ps.iter().find(|p| p.name == "WAVE").expect("the waveform");
        assert_eq!(wave.steps as usize, BASE_FUNCS.len());
        assert_eq!(wave.points[3].1, "saw");
    }
}
