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
    (0..inst.param_count())
        .map(|id| {
            let (steps, points) = inst.param_steps(id);
            let unit = inst.param_label(id);
            PluginParam {
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
