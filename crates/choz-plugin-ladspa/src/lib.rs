//! LADSPA and DSSI plugin hosting for choz.
//!
//! Both formats share the LADSPA C ABI: a `.so` exports `ladspa_descriptor`
//! (effects) and/or `dssi_descriptor` (synths, which add `run_synth` and ALSA
//! sequencer events on top of the same descriptor). One dlopen per file, one
//! instance per rack slot.
//!
//! ```text
//! choz-plugin-ladspa
//!   ├── scan_directory(dir)  ← dlopen + enumerate descriptors → PluginInfo
//!   ├── LadspaEffect         ← choz_ports::FxProcessor (audio → audio)
//!   └── DssiInstrument       ← choz_ports::AudioSource  (notes → audio)
//! ```

pub mod abi;

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libloading::Library;

use abi::*;
use choz_ports::{AudioSource, FxProcessor, PluginParam};

/// Cap on MIDI events queued between two blocks (RT-safe: drop when full).
const MAX_PENDING_MIDI: usize = 256;

/// A discovered LADSPA/DSSI plugin: which file, which descriptor inside it, and
/// what it does.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub path: PathBuf,
    /// Descriptor index inside the file — one `.so` can export many plugins.
    pub index: u32,
    /// Unique label (the LADSPA `Label`), used as choz's plugin id.
    pub label: String,
    /// Human-readable name.
    pub name: String,
    /// True when the file exports a DSSI synth for this descriptor.
    pub is_instrument: bool,
    pub audio_inputs: usize,
    pub audio_outputs: usize,
    pub params: Vec<PluginParam>,
}

/// Every LADSPA/DSSI plugin under `dir`. Each `.so` is dlopened once; a file
/// that isn't a plugin (or crashes on load) is skipped.
pub fn scan_directory(dir: &Path) -> Vec<PluginInfo> {
    let mut out = Vec::new();
    scan_recursive(dir, 0, &mut out);
    out
}

fn scan_recursive(dir: &Path, depth: usize, out: &mut Vec<PluginInfo>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            scan_recursive(&path, depth + 1, out);
        } else if path.extension().is_some_and(|e| e == "so") {
            out.extend(describe(&path));
        }
    }
}

/// Every plugin exported by one `.so`.
pub fn describe(path: &Path) -> Vec<PluginInfo> {
    let Ok(lib) = (unsafe { Library::new(path) }) else { return Vec::new() };
    let mut out = Vec::new();
    // DSSI first: its descriptors carry a LADSPA one, and knowing a plugin is a
    // synth is what decides which side of the UI it shows up on.
    let dssi_labels = unsafe { enumerate_dssi(&lib, path, &mut out) };
    unsafe { enumerate_ladspa(&lib, path, &dssi_labels, &mut out) };
    out
}

/// # Safety
/// `lib` must be a loaded plugin library; the descriptors it returns are only
/// read (never kept) while `lib` is alive.
unsafe fn enumerate_dssi(lib: &Library, path: &Path, out: &mut Vec<PluginInfo>) -> Vec<String> {
    let mut labels = Vec::new();
    let Ok(entry) = (unsafe { lib.get::<DssiDescriptorFn>(DSSI_DESCRIPTOR_SYM) }) else {
        return labels;
    };
    for i in 0.. {
        let d = unsafe { entry(i) };
        if d.is_null() {
            break;
        }
        let ladspa = unsafe { (*d).ladspa };
        if ladspa.is_null() {
            continue;
        }
        // A DSSI descriptor without run_synth is just an effect.
        let is_synth = unsafe { (*d).run_synth.is_some() };
        if let Some(info) = unsafe { info_from(ladspa, path, i as u32, is_synth) } {
            labels.push(info.label.clone());
            out.push(info);
        }
    }
    labels
}

/// # Safety
/// Same contract as [`enumerate_dssi`].
unsafe fn enumerate_ladspa(
    lib: &Library,
    path: &Path,
    already: &[String],
    out: &mut Vec<PluginInfo>,
) {
    let Ok(entry) = (unsafe { lib.get::<LadspaDescriptorFn>(LADSPA_DESCRIPTOR_SYM) }) else {
        return;
    };
    for i in 0.. {
        let d = unsafe { entry(i) };
        if d.is_null() {
            break;
        }
        if let Some(info) = unsafe { info_from(d, path, i as u32, false) } {
            // Already listed through the DSSI entry point.
            if !already.contains(&info.label) {
                out.push(info);
            }
        }
    }
}

/// # Safety
/// `d` must be a valid descriptor from a library that is still loaded.
unsafe fn info_from(
    d: *const LADSPA_Descriptor,
    path: &Path,
    index: u32,
    is_instrument: bool,
) -> Option<PluginInfo> {
    let label = unsafe { cstr(( *d).label) }?;
    let name = unsafe { cstr((*d).name) }.unwrap_or_else(|| label.clone());
    let ports = unsafe { port_table(d) };
    Some(PluginInfo {
        path: path.to_path_buf(),
        index,
        label,
        name,
        is_instrument,
        audio_inputs: ports.audio_in.len(),
        audio_outputs: ports.audio_out.len(),
        params: ports.params,
    })
}

/// # Safety
/// `p` must be a NUL-terminated C string or null.
unsafe fn cstr(p: *const std::os::raw::c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Ports of one descriptor, split by what they carry.
struct PortTable {
    audio_in: Vec<usize>,
    audio_out: Vec<usize>,
    /// Control input ports = the plugin's parameters (`id` is the port index).
    params: Vec<PluginParam>,
    /// Every port index that is neither audio nor a parameter (control outputs);
    /// they still need a buffer connected or the plugin writes through a null.
    other: Vec<usize>,
}

/// # Safety
/// `d` must be a valid descriptor from a loaded library.
unsafe fn port_table(d: *const LADSPA_Descriptor) -> PortTable {
    // The sample rate only scales bounds for display; the instance re-reads
    // them at its real rate.
    const NOMINAL_SR: u32 = 48_000;
    let n = unsafe { (*d).port_count } as usize;
    let descs = unsafe { (*d).port_descriptors };
    let hints = unsafe { (*d).port_range_hints };
    let names = unsafe { (*d).port_names };
    let mut t = PortTable {
        audio_in: Vec::new(),
        audio_out: Vec::new(),
        params: Vec::new(),
        other: Vec::new(),
    };
    if descs.is_null() {
        return t;
    }
    for i in 0..n {
        let desc = unsafe { *descs.add(i) };
        let is_input = desc & LADSPA_PORT_INPUT != 0;
        match (desc & LADSPA_PORT_AUDIO != 0, is_input) {
            (true, true) => t.audio_in.push(i),
            (true, false) => t.audio_out.push(i),
            (false, true) if !hints.is_null() => {
                let hint = unsafe { *hints.add(i) };
                let (min, max) = bounds(&hint, NOMINAL_SR);
                let name = if names.is_null() {
                    None
                } else {
                    unsafe { cstr(*names.add(i)) }
                };
                t.params.push(PluginParam {
                    id: i as u32,
                    name: name.unwrap_or_else(|| format!("P{i}")),
                    min: min as f64,
                    max: max as f64,
                    default: default_for(&hint, NOMINAL_SR) as f64,
                });
            }
            _ => t.other.push(i),
        }
    }
    t
}

/// Parameters of one plugin, for the UI. Non-RT (dlopens the file).
pub fn read_params(path: &Path, label: &str) -> Vec<PluginParam> {
    describe(path)
        .into_iter()
        .find(|p| p.label == label)
        .map(|p| p.params)
        .unwrap_or_default()
}

// ─── Instance ───────────────────────────────────────────────────────────────

/// A live plugin: the handle, its port buffers, and the library kept alive.
struct Instance {
    handle: LADSPA_Handle,
    descriptor: *const LADSPA_Descriptor,
    /// Set when this instance came from a DSSI synth descriptor.
    dssi: *const DSSI_Descriptor,
    _lib: Arc<Library>,
    block_size: usize,
    /// One f32 cell per port (control ports read theirs; others get a scratch).
    control_values: Vec<f32>,
    /// Per-port audio buffer; empty for non-audio ports.
    audio_bufs: Vec<Vec<f32>>,
    audio_in: Vec<usize>,
    audio_out: Vec<usize>,
    params: Vec<PluginParam>,
    pending_midi: Vec<[u8; 3]>,
    /// Scratch the RT thread converts `pending_midi` into; pre-allocated.
    events: Vec<snd_seq_event_t>,
    activated: bool,
}

// SAFETY: an Instance owns its plugin handle exclusively — it is created on the
// UI thread, then moved to the audio thread and never shared.
unsafe impl Send for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            if self.activated {
                if let Some(deactivate) = (*self.descriptor).deactivate {
                    deactivate(self.handle);
                }
            }
            if let Some(cleanup) = (*self.descriptor).cleanup {
                cleanup(self.handle);
            }
        }
    }
}

/// Load `label` from `path` and activate it. `want_synth` picks the DSSI
/// descriptor (and fails when the plugin has no `run_synth`).
fn build(path: &Path, label: &str, sample_rate: u32, block: u32, want_synth: bool) -> Result<Instance> {
    let lib = Arc::new(
        unsafe { Library::new(path) }.with_context(|| format!("dlopen {}", path.display()))?,
    );

    // Find the descriptor whose label matches, through the right entry point.
    let (descriptor, dssi) = unsafe { find_descriptor(&lib, label, want_synth)? };

    let ports = unsafe { port_table(descriptor) };
    let params: Vec<PluginParam> = ports
        .params
        .iter()
        .map(|p| {
            // Re-read the bounds at the real sample rate: some ports are
            // expressed as a fraction of it.
            let hint = unsafe { *(*descriptor).port_range_hints.add(p.id as usize) };
            let (min, max) = bounds(&hint, sample_rate);
            PluginParam {
                min: min as f64,
                max: max as f64,
                default: default_for(&hint, sample_rate) as f64,
                ..p.clone()
            }
        })
        .collect();

    let nports = unsafe { (*descriptor).port_count } as usize;
    let mut audio_bufs = vec![Vec::<f32>::new(); nports];
    for &i in ports.audio_in.iter().chain(ports.audio_out.iter()) {
        audio_bufs[i] = vec![0.0; block as usize];
    }
    let mut control_values = vec![0.0f32; nports];
    for p in &params {
        control_values[p.id as usize] = p.default as f32;
    }

    let instantiate = unsafe { (*descriptor).instantiate }
        .ok_or_else(|| anyhow::anyhow!("{label} has no instantiate fn"))?;
    let handle = unsafe { instantiate(descriptor, sample_rate as std::os::raw::c_ulong) };
    if handle.is_null() {
        bail!("instantiate returned null for {label}");
    }

    let mut inst = Instance {
        handle,
        descriptor,
        dssi,
        _lib: lib,
        block_size: block as usize,
        control_values,
        audio_bufs,
        audio_in: ports.audio_in,
        audio_out: ports.audio_out,
        params,
        pending_midi: Vec::with_capacity(MAX_PENDING_MIDI),
        events: Vec::with_capacity(MAX_PENDING_MIDI),
        activated: false,
    };
    inst.connect_all();
    if let Some(activate) = unsafe { (*descriptor).activate } {
        unsafe { activate(handle) };
    }
    inst.activated = true;
    Ok(inst)
}

/// # Safety
/// `lib` must stay loaded for as long as the returned descriptor is used.
unsafe fn find_descriptor(
    lib: &Library,
    label: &str,
    want_synth: bool,
) -> Result<(*const LADSPA_Descriptor, *const DSSI_Descriptor)> {
    if let Ok(entry) = unsafe { lib.get::<DssiDescriptorFn>(DSSI_DESCRIPTOR_SYM) } {
        for i in 0.. {
            let d = unsafe { entry(i) };
            if d.is_null() {
                break;
            }
            let ladspa = unsafe { (*d).ladspa };
            if ladspa.is_null() {
                continue;
            }
            if unsafe { cstr((*ladspa).label) }.as_deref() == Some(label) {
                if want_synth && unsafe { (*d).run_synth.is_none() } {
                    bail!("DSSI plugin {label} has no run_synth; not an instrument");
                }
                return Ok((ladspa, d));
            }
        }
    }
    if want_synth {
        bail!("{label} is not a DSSI instrument");
    }
    let entry = unsafe { lib.get::<LadspaDescriptorFn>(LADSPA_DESCRIPTOR_SYM) }
        .context("missing ladspa_descriptor symbol")?;
    for i in 0.. {
        let d = unsafe { entry(i) };
        if d.is_null() {
            break;
        }
        if unsafe { cstr((*d).label) }.as_deref() == Some(label) {
            return Ok((d, std::ptr::null()));
        }
    }
    bail!("plugin {label} not exported by this file")
}

impl Instance {
    fn connect_all(&mut self) {
        let Some(connect) = (unsafe { (*self.descriptor).connect_port }) else { return };
        for i in 0..self.control_values.len() {
            let ptr: *mut f32 = if self.audio_bufs[i].is_empty() {
                &mut self.control_values[i]
            } else {
                self.audio_bufs[i].as_mut_ptr()
            };
            unsafe { connect(self.handle, i as std::os::raw::c_ulong, ptr) };
        }
    }

    /// Set control port `index` (into `params`) from a 0..1 knob position.
    /// RT-safe: one f32 write the plugin picks up on the next `run`.
    fn set_param_norm(&mut self, index: usize, value: f32) {
        let Some(info) = self.params.get(index) else { return };
        let plain = info.plain(value.clamp(0.0, 1.0) as f64) as f32;
        if let Some(cell) = self.control_values.get_mut(info.id as usize) {
            *cell = plain;
        }
    }

    fn queue_midi(&mut self, bytes: [u8; 3]) {
        if self.pending_midi.len() < MAX_PENDING_MIDI {
            self.pending_midi.push(bytes);
        }
    }

    /// Run `frames` of audio. DSSI synths get the queued MIDI as ALSA events.
    fn run(&mut self, frames: usize) {
        if !self.dssi.is_null() {
            self.events.clear();
            for msg in &self.pending_midi {
                if let Some(ev) = snd_seq_event_t::from_midi(*msg, 0) {
                    self.events.push(ev);
                }
            }
            self.pending_midi.clear();
            if let Some(run_synth) = unsafe { (*self.dssi).run_synth } {
                unsafe {
                    run_synth(
                        self.handle,
                        frames as std::os::raw::c_ulong,
                        self.events.as_mut_ptr(),
                        self.events.len() as std::os::raw::c_ulong,
                    )
                };
                return;
            }
        }
        if let Some(run) = unsafe { (*self.descriptor).run } {
            unsafe { run(self.handle, frames as std::os::raw::c_ulong) };
        }
    }

    /// One block of interleaved stereo through the plugin, in place.
    fn process_interleaved(&mut self, block: &mut [f32], wet: f32) {
        let frames = (block.len() / 2).min(self.block_size);
        let mono_in = self.audio_in.len() == 1;
        for (ch, &pi) in self.audio_in.iter().enumerate() {
            let buf = &mut self.audio_bufs[pi];
            for (f, slot) in buf.iter_mut().enumerate().take(frames) {
                *slot = if mono_in {
                    (block[f * 2] + block[f * 2 + 1]) * 0.5
                } else {
                    block[f * 2 + ch.min(1)]
                };
            }
        }
        self.run(frames);
        let dry = 1.0 - wet;
        for f in 0..frames {
            let (l, r) = self.out_frame(f);
            block[f * 2] = block[f * 2] * dry + l * wet;
            block[f * 2 + 1] = block[f * 2 + 1] * dry + r * wet;
        }
    }

    /// Render a block as an instrument: audio inputs silent, output written out.
    fn render_block(&mut self, out: &mut [f32]) -> usize {
        let frames = (out.len() / 2).min(self.block_size);
        if frames == 0 {
            return 0;
        }
        for &pi in &self.audio_in {
            for v in self.audio_bufs[pi].iter_mut().take(frames) {
                *v = 0.0;
            }
        }
        self.run(frames);
        for f in 0..frames {
            let (l, r) = self.out_frame(f);
            out[f * 2] = l;
            out[f * 2 + 1] = r;
        }
        frames
    }

    /// Frame `f` of the plugin's output as a stereo pair. A mono plugin is
    /// duplicated; non-finite output is dropped rather than passed on.
    fn out_frame(&self, f: usize) -> (f32, f32) {
        let (l, r) = match self.audio_out.len() {
            0 => (0.0, 0.0),
            1 => {
                let v = self.audio_bufs[self.audio_out[0]][f];
                (v, v)
            }
            _ => (
                self.audio_bufs[self.audio_out[0]][f],
                self.audio_bufs[self.audio_out[1]][f],
            ),
        };
        if l.is_finite() && r.is_finite() { (l, r) } else { (0.0, 0.0) }
    }
}

// ─── Effect / instrument wrappers ───────────────────────────────────────────

/// A live LADSPA (or DSSI) audio effect in a slot's FX chain.
pub struct LadspaEffect {
    inst: Instance,
    wet: f32,
}

impl LadspaEffect {
    /// Load the plugin `label` from `path`. `None` on any failure — a broken
    /// plugin must never take the app down.
    pub fn build(path: &Path, label: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        match build(path, label, sample_rate, max_block, false) {
            Ok(inst) if inst.audio_out.is_empty() => {
                eprintln!("choz: LADSPA {label} has no audio output; not an effect");
                None
            }
            Ok(inst) => Some(Self { inst, wet: 1.0 }),
            Err(e) => {
                eprintln!("choz: LADSPA {label}: {e}");
                None
            }
        }
    }
}

impl FxProcessor for LadspaEffect {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let chunk = self.inst.block_size;
        let wet = self.wet;
        for block in buf.chunks_mut(chunk * 2) {
            self.inst.process_interleaved(block, wet);
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
        self.inst
            .params
            .iter()
            .map(|p| choz_ports::FxParam::new("param", 0.0, p.min as f32, p.max as f32, ""))
            .collect()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param_norm(index, value);
    }
}

/// A live DSSI instrument in a rack slot: notes in, interleaved stereo out.
pub struct DssiInstrument {
    inst: Instance,
}

impl DssiInstrument {
    /// Load the DSSI synth `label` from `path`. `None` on any failure.
    pub fn build(path: &Path, label: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        match build(path, label, sample_rate, max_block, true) {
            Ok(inst) => Some(Self { inst }),
            Err(e) => {
                eprintln!("choz: DSSI {label}: {e}");
                None
            }
        }
    }
}

impl AudioSource for DssiInstrument {
    fn render(&mut self, output: &mut [f32], _sample_rate: u32) -> usize {
        let mut done = 0;
        let total = output.len() / 2;
        while done < total {
            let n = self.inst.render_block(&mut output[done * 2..]);
            if n == 0 {
                break;
            }
            done += n;
        }
        done
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.inst.queue_midi([0x90, note & 0x7F, velocity & 0x7F]);
    }

    fn note_off(&mut self, note: u8) {
        self.inst.queue_midi([0x80, note & 0x7F, 0]);
    }

    fn control_change(&mut self, cc: u8, value: u8) {
        self.inst.queue_midi([0xB0, cc & 0x7F, value & 0x7F]);
    }

    fn pitch_bend(&mut self, value: u16) {
        let v = value.min(16383);
        self.inst.queue_midi([0xE0, (v & 0x7F) as u8, (v >> 7) as u8]);
    }

    fn program_change(&mut self, bank: u8, preset: u8) {
        // DSSI has its own program call, and it is not an RT-safe one for every
        // plugin — but it is what the format offers, and the alternative is no
        // program change at all.
        if !self.inst.dssi.is_null() {
            if let Some(select) = unsafe { (*self.inst.dssi).select_program } {
                unsafe {
                    select(
                        self.inst.handle,
                        bank as std::os::raw::c_ulong,
                        preset as std::os::raw::c_ulong,
                    )
                };
            }
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param_norm(index, value);
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}
