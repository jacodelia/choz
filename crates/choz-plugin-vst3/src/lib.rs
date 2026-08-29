//! VST3 plugin hosting for choz.
//!
//! ```text
//! choz-plugin-vst3
//!   ├── scan_directory(dir) ← `.vst3` bundles + their factory class info
//!   ├── Vst3Effect          ← choz_ports::FxProcessor (audio → audio)
//!   └── Vst3Instrument      ← choz_ports::AudioSource (notes → audio)
//! ```
//!
//! The COM plumbing lives in [`host`], ported from seqterm's
//! `seqterm-plugin-vst3`; the native window (`IPlugView` + the Linux run loop)
//! lives in [`editor`].

pub mod editor;
pub mod host;
pub mod presets;

use std::path::{Path, PathBuf};

use choz_ports::{AudioSource, EditorHandle, FxProcessor, PluginParam, TouchHandle};
use host::Vst3RealInstance;

/// A discovered VST3 plugin bundle.
#[derive(Debug, Clone)]
pub struct Vst3PluginInfo {
    /// Absolute path to the `.vst3` bundle directory.
    pub path: PathBuf,
    pub name: String,
    pub vendor: String,
    /// True when the factory declares the class as an instrument.
    pub is_instrument: bool,
}

/// Every `.vst3` bundle under `dir`. The factory is read (not instantiated) to
/// learn each plugin's name and whether it is an instrument.
pub fn scan_directory(dir: &Path) -> Vec<Vst3PluginInfo> {
    let mut out = Vec::new();
    scan_recursive(dir, 0, &mut out);
    out
}

fn scan_recursive(dir: &Path, depth: usize, out: &mut Vec<Vst3PluginInfo>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if path.extension().is_some_and(|e| e == "vst3") {
            out.push(describe(&path));
        } else {
            scan_recursive(&path, depth + 1, out);
        }
    }
}

/// Bundle metadata. Falls back to the bundle name when the factory can't be
/// read — the plugin still shows up, it just isn't classified.
pub fn describe(path: &Path) -> Vst3PluginInfo {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown".into());
    match host::factory_info(path) {
        Some(info) => Vst3PluginInfo {
            path: path.to_path_buf(),
            name: if info.name.is_empty() {
                name
            } else {
                info.name
            },
            vendor: info.vendor,
            is_instrument: info.is_instrument,
        },
        None => Vst3PluginInfo {
            path: path.to_path_buf(),
            name,
            vendor: String::new(),
            is_instrument: false,
        },
    }
}

/// Parameters exposed by the plugin's edit controller. Non-RT: this loads the
/// plugin, so it runs once when an effect or instrument is added.
pub fn read_params(path: &Path, _id: &str) -> Vec<PluginParam> {
    let Ok(inst) = Vst3RealInstance::load(path, 48_000, 64) else {
        return Vec::new();
    };
    // What the plugin says its sections are, once for the whole list — see
    // `Vst3RealInstance::param_groups`.
    let groups = inst.param_groups();
    (0..inst.param_count())
        .map(|id| {
            let (steps, points) = inst.param_steps(id);
            // A plugin that reports no steps has not said "continuous" — Surge
            // XT reports zero for all 800 of its parameters, switches and mode
            // lists included. What it does answer is what a value *reads* as,
            // so ask: a control whose range reads as a handful of words is a
            // switch or a list of positions, and gets drawn as one.
            let (steps, points) = if steps == 0 {
                steps_from_labels(&inst, id).unwrap_or((steps, points))
            } else {
                (steps, points)
            };
            let unit = inst.param_label(id);
            PluginParam {
                group: groups.get(id as usize).cloned().flatten(),
                id,
                name: inst.param_name(id),
                // VST3 parameters are normalised by definition.
                min: 0.0,
                max: 1.0,
                default: inst.get_param(id) as f64,
                steps,
                unit: (!unit.is_empty()).then_some(unit),
                points,
            }
        })
        .collect()
}

/// The positions of a parameter that reports none, read off the plugin's own
/// words. `None` when it reads as anything but a list of names, which leaves
/// the parameter exactly as the plugin described it.
///
/// Surge XT reports zero steps for all 800 of its parameters, `A Play Mode`
/// included, and still renders "Poly"/"Mono"/"Mono ST"/"Latch" for it. The
/// only place those names exist is `getParamStringByValue`, so the range is
/// swept: labels that hold over a run of probes and then change are positions,
/// and each one is placed at the middle of its run — the safest point to send
/// back, since the edges of a plateau are exactly where a rounding difference
/// lands on the neighbour.
///
/// Rejected, and left a knob: anything that reads as a number ("0.50",
/// "-12.0 dB"), a label that comes back in two separate runs (which is a
/// waveform table being sampled too coarsely, not a list), and more positions
/// than are worth stepping through one by one.
fn steps_from_labels(inst: &host::Vst3RealInstance, id: u32) -> Option<choz_ports::Positions> {
    // Enough to separate the handful of positions a mode switch has without
    // asking a plugin for 800 × N strings on load. When every probe reads
    // differently the sweep was too coarse to have found the plateaus — Surge
    // XT's filter type has more entries than that — so that one parameter is
    // swept again finely, and only that one pays for it.
    steps_from_labels_at(inst, id, 17).and_then(|(sweep, out)| {
        if sweep {
            steps_from_labels_at(inst, id, 65).map(|(_, out)| out)
        } else {
            Some(out)
        }
    })
}

/// One sweep of `probes` points.
fn steps_from_labels_at(
    inst: &host::Vst3RealInstance,
    id: u32,
    probes: usize,
) -> Option<(bool, choz_ports::Positions)> {
    let probes = probes.max(3);
    let shown: Vec<String> = (0..probes)
        .map(|k| inst.param_display_at(id, k as f64 / (probes - 1) as f64))
        .collect();
    choz_ports::positions_from_labels(&shown, host::MAX_NAMED_STEPS)
}

/// A live VST3 audio effect in a slot's FX chain.
pub struct Vst3Effect {
    inst: Vst3RealInstance,
    /// Scratch for the dry signal of one chunk, so the plugin can write into
    /// the block while the mix still has the original. Pre-allocated.
    dry: Vec<f32>,
    wet: f32,
}

impl Vst3Effect {
    /// Load the VST3 effect at `path`. `None` on any failure.
    pub fn build(path: &Path, sample_rate: u32, max_block: u32) -> Option<Self> {
        match Vst3RealInstance::load(path, sample_rate, max_block) {
            Ok(inst) => Some(Self {
                inst,
                dry: vec![0.0; max_block as usize * 2],
                wet: 1.0,
            }),
            Err(e) => {
                eprintln!("choz: VST3 {}: {e}", path.display());
                None
            }
        }
    }
}

impl FxProcessor for Vst3Effect {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let chunk = self.dry.len() / 2;
        for block in buf.chunks_mut(chunk * 2) {
            let n = block.len();
            self.dry[..n].copy_from_slice(block);
            self.inst.render_with_input(&self.dry[..n], block);
            let dry_gain = 1.0 - self.wet;
            for (i, s) in block.iter_mut().enumerate() {
                let wet = if s.is_finite() { *s } else { 0.0 };
                *s = self.dry[i] * dry_gain + wet * self.wet;
            }
        }
    }

    fn reset(&mut self) {}

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    /// The trait's descriptor wants a `'static` name, which a plugin's dynamic
    /// names can't provide — the UI reads them with [`read_params`] instead.
    /// What matters here is the count.
    fn params(&self) -> Vec<choz_ports::FxParam> {
        (0..self.inst.param_count())
            .map(|_| choz_ports::FxParam::new("param", 0.0, 0.0, 1.0, ""))
            .collect()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param(index as u32, value.clamp(0.0, 1.0));
    }

    fn editor(&self) -> Option<EditorHandle> {
        self.inst.editor()
    }

    fn param_touch(&self) -> Option<TouchHandle> {
        Some(std::sync::Arc::new(self.inst.edit_feed()) as TouchHandle)
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        self.inst.state()
    }
}

/// A live VST3 instrument in a rack slot: notes in, interleaved stereo out.
pub struct Vst3Instrument {
    inst: Vst3RealInstance,
}

impl Vst3Instrument {
    /// Load the VST3 instrument at `path`. `None` on any failure.
    pub fn build(path: &Path, sample_rate: u32, max_block: u32) -> Option<Self> {
        match Vst3RealInstance::load(path, sample_rate, max_block) {
            Ok(inst) => Some(Self { inst }),
            Err(e) => {
                eprintln!("choz: VST3 {}: {e}", path.display());
                None
            }
        }
    }
}

impl AudioSource for Vst3Instrument {
    fn render(&mut self, output: &mut [f32], _sample_rate: u32) -> usize {
        let mut done = 0;
        let total = output.len() / 2;
        while done < total {
            let n = self.inst.render(&mut output[done * 2..]);
            if n == 0 {
                break;
            }
            done += n;
        }
        done
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.inst.note_on(0, note, velocity);
    }

    fn note_off(&mut self, note: u8) {
        self.inst.note_off(0, note);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param(index as u32, value.clamp(0.0, 1.0));
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }

    fn editor(&self) -> Option<EditorHandle> {
        self.inst.editor()
    }

    fn param_touch(&self) -> Option<TouchHandle> {
        Some(std::sync::Arc::new(self.inst.edit_feed()) as TouchHandle)
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        self.inst.state()
    }

    fn presets(&self) -> Option<choz_ports::PresetsHandle> {
        self.inst.presets()
    }
}
