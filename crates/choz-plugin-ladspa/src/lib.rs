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
pub mod rdf;

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
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
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
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
    let Ok(lib) = (unsafe { Library::new(path) }) else {
        return Vec::new();
    };
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
        // A synth is one that can be *run* as a synth. `run_multiple_synths`
        // counts: it is what a plugin exports when one engine serves several
        // instances (fluidsynth-dssi does exactly this, and calling it with a
        // single handle is what the DSSI spec says a host may do). Looking only
        // at `run_synth` filed FluidSynth-DSSI as an effect and left it
        // unloadable.
        let is_synth = unsafe { (*d).run_synth.is_some() || (*d).run_multiple_synths.is_some() };
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
    let label = unsafe { cstr((*d).label) }?;
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

/// How many positions a port has, once the metadata has had its say.
///
/// [`steps_of`] answers from the hint alone, which is all the ABI carries. A
/// port the RDF names positions for has exactly as many as it named — the same
/// rule LV2 uses for `lv2:enumeration` — and that has to win: a caps port whose
/// three named settings sit at 0, 50 and 100 is three positions, not the
/// hundred and one an integer hint would claim, and naming three of a hundred
/// and one would draw two names and ninety-nine numbers.
/// The named positions a port can actually be set to.
///
/// Metadata drifts from the plugin it describes: caps' file names four modes
/// for `Compress/mode` — the last at 3 — and the port the binary declares runs
/// 0..2. A name for a value nothing can select is worse than no name, because
/// it is the one a list would offer and the knob could never reach.
fn usable_points(points: Vec<(f64, String)>, min: f32, max: f32) -> Vec<(f64, String)> {
    let (lo, hi) = (min.min(max) as f64, min.max(max) as f64);
    points
        .into_iter()
        .filter(|(v, _)| *v >= lo - 1e-6 && *v <= hi + 1e-6)
        .collect()
}

fn steps_with_names(
    hint: &LADSPA_PortRangeHint,
    min: f32,
    max: f32,
    points: &[(f64, String)],
) -> u32 {
    let hinted = steps_of(hint, min, max);
    match points.len() as u32 {
        // Nothing named: the hint is all there is.
        0 => hinted,
        // The hint says nothing (a plain float port) and the file named the
        // positions: those are the positions, the way `lv2:enumeration` works.
        named if hinted == 0 => named,
        // Both agree.
        named if named == hinted => named,
        // **They disagree, so the file named only some of them.** Keep the
        // hint's count: swh's `gate` runs −1..1 with three integer settings and
        // its metadata names two, and taking the file's word turned a
        // three-position port into a switch — whose two ends are −1 and 1, so
        // the middle setting the plugin calls "gate" could not be reached at
        // all. The names still label whatever the knob lands on, which is what
        // `PluginParam::label_for` is for.
        _ => hinted,
    }
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
    // What the metadata files address a port by. See [`crate::rdf`].
    let unique = unsafe { (*d).unique_id };
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
                let name = name.unwrap_or_else(|| format!("P{i}"));
                let points =
                    usable_points(crate::rdf::points_for(unique, i as u32, &name), min, max);
                t.params.push(PluginParam {
                    id: i as u32,
                    name,
                    min: min as f64,
                    max: max as f64,
                    default: default_for(&hint, NOMINAL_SR) as f64,
                    steps: steps_with_names(&hint, min, max, &points),
                    // LADSPA has no units. The step *names* are not in the ABI
                    // either — they are in the metadata beside the plugin, see
                    // [`crate::rdf`].
                    points,
                    ..PluginParam::default()
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
    /// `(bank, program, name)` as the plugin listed them at load time. DSSI
    /// plugins build this list once (hexter's 32 patches per bank, the
    /// SoundFont FluidSynth-DSSI was given) and it does not change under us.
    programs: Vec<(u32, u32, String)>,
    /// The program the UI last asked for, picked up by the audio thread.
    ///
    /// A shared cell rather than a lock: `select_program` has to run where the
    /// instance lives, which is the audio thread, and the UI has no other way
    /// to reach it. One `AtomicU64` — `dirty | bank | program` — costs a
    /// relaxed load per block and cannot block the callback.
    program_request: Arc<AtomicU64>,
    activated: bool,
}

/// Set on [`Instance::program_request`] when the UI has asked for a program;
/// cleared by the audio thread once it has been selected.
const PROGRAM_REQUESTED: u64 = 1 << 63;

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
fn build(
    path: &Path,
    label: &str,
    sample_rate: u32,
    block: u32,
    want_synth: bool,
) -> Result<Instance> {
    let lib = Arc::new(
        unsafe { Library::new(path) }.with_context(|| format!("dlopen {}", path.display()))?,
    );
    keep_loaded(&lib);

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
            // The bounds are re-read at the real rate, so a point that was in
            // range at the nominal one may not be at this one.
            let points = usable_points(p.points.clone(), min, max);
            PluginParam {
                min: min as f64,
                max: max as f64,
                default: default_for(&hint, sample_rate) as f64,
                steps: steps_with_names(&hint, min, max, &points),
                points,
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
        programs: Vec::new(),
        program_request: Arc::new(AtomicU64::new(0)),
        activated: false,
    };
    inst.connect_all();
    if let Some(activate) = unsafe { (*descriptor).activate } {
        unsafe { activate(handle) };
    }
    inst.activated = true;
    inst.programs = unsafe { read_programs(&inst) };
    Ok(inst)
}

/// The programs a DSSI plugin declares, asked for one by one until it stops
/// answering. Non-RT: it happens once, at load.
///
/// # Safety
/// `inst` must hold a live handle and, if any, a live DSSI descriptor.
unsafe fn read_programs(inst: &Instance) -> Vec<(u32, u32, String)> {
    if inst.dssi.is_null() {
        return Vec::new();
    }
    let Some(get) = (unsafe { (*inst.dssi).get_program }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let d = unsafe { get(inst.handle, i) };
        if d.is_null() {
            break;
        }
        let name = unsafe { cstr((*d).name) }.unwrap_or_default();
        out.push((
            unsafe { (*d).bank } as u32,
            unsafe { (*d).program } as u32,
            name,
        ));
        // A plugin that keeps answering forever is a plugin with a bug, and a
        // host that keeps asking is a host that hangs.
        if out.len() >= 4096 {
            break;
        }
    }
    out
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
                if want_synth
                    && unsafe { (*d).run_synth.is_none() && (*d).run_multiple_synths.is_none() }
                {
                    bail!("DSSI plugin {label} has no way to run as a synth; not an instrument");
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
        let Some(connect) = (unsafe { (*self.descriptor).connect_port }) else {
            return;
        };
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
        let Some(info) = self.params.get(index) else {
            return;
        };
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

    /// Send one DSSI `configure` key/value. Returns the plugin's error string,
    /// if it complained.
    ///
    /// This is how a DSSI synth is told the things that are not parameters —
    /// FluidSynth-DSSI takes `load` with the path to a SoundFont, and without
    /// it the plugin runs and stays silent, which is exactly how it looked
    /// here. Not RT-safe (it reads files): UI thread only, before or between
    /// blocks.
    fn configure(&self, key: &str, value: &str) -> Option<String> {
        if self.dssi.is_null() {
            return Some("not a DSSI plugin".into());
        }
        let configure = unsafe { (*self.dssi).configure }?;
        let (k, v) = (
            std::ffi::CString::new(key).ok()?,
            std::ffi::CString::new(value).ok()?,
        );
        // SAFETY: live handle; the plugin copies out of both strings during the
        // call. The message it may return is ours to free — `free`, because the
        // plugin allocated it with `malloc`.
        let msg = unsafe { configure(self.handle, k.as_ptr(), v.as_ptr()) };
        if msg.is_null() {
            return None;
        }
        let out = unsafe { cstr(msg) };
        unsafe { libc_free(msg as *mut std::os::raw::c_void) };
        out
    }

    /// Run `frames` of audio. DSSI synths get the queued MIDI as ALSA events.
    fn run(&mut self, frames: usize) {
        if !self.dssi.is_null() {
            // A program the UI asked for, applied here because `select_program`
            // has to run where the instance is.
            let req = self.program_request.load(Ordering::Relaxed);
            if req & PROGRAM_REQUESTED != 0 {
                self.program_request.store(0, Ordering::Relaxed);
                if let Some(select) = unsafe { (*self.dssi).select_program } {
                    let bank = ((req >> 32) & 0x7FFF_FFFF) as std::os::raw::c_ulong;
                    let program = (req & 0xFFFF_FFFF) as std::os::raw::c_ulong;
                    unsafe { select(self.handle, bank, program) };
                }
            }
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
            // The other half of the format: one call for a group of instances
            // sharing an engine. choz gives each tab its own instance, so the
            // group is always of one — which the spec allows, and which is the
            // only way to run FluidSynth-DSSI at all.
            if let Some(run_multi) = unsafe { (*self.dssi).run_multiple_synths } {
                let mut handle = self.handle;
                let mut events = self.events.as_mut_ptr();
                let mut count = self.events.len() as std::os::raw::c_ulong;
                unsafe {
                    run_multi(
                        1,
                        &mut handle,
                        frames as std::os::raw::c_ulong,
                        &mut events,
                        &mut count,
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
        if l.is_finite() && r.is_finite() {
            (l, r)
        } else {
            (0.0, 0.0)
        }
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

/// A DSSI synth's own programs.
///
/// Listed once at load (`get_program`), selected through the instance's shared
/// request cell — the audio thread is where `select_program` has to happen, and
/// it is the only side that can still reach the plugin once the source has
/// moved there.
struct DssiPresets {
    programs: Vec<(u32, u32, String)>,
    request: Arc<AtomicU64>,
}

impl choz_ports::PluginPresets for DssiPresets {
    fn list(&self) -> Vec<choz_ports::PresetEntry> {
        self.programs
            .iter()
            .map(|(bank, program, name)| choz_ports::PresetEntry {
                name: if name.trim().is_empty() {
                    format!("Program {program}")
                } else {
                    name.clone()
                },
                // DSSI banks are numbers, not names: the picker files them as
                // "Bank 0" so its chips still say something.
                category: format!("Bank {bank}"),
                key: format!("{bank}:{program}"),
            })
            .collect()
    }

    fn load(&self, key: &str) {
        let Some((bank, program)) = key.split_once(':') else {
            return;
        };
        let (Ok(bank), Ok(program)) = (bank.parse::<u32>(), program.parse::<u32>()) else {
            return;
        };
        self.request.store(
            PROGRAM_REQUESTED | ((bank as u64) << 32) | program as u64,
            Ordering::Relaxed,
        );
    }
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

    /// Send a DSSI `configure` key/value to this synth. See
    /// [`Instance::configure`]: it is how FluidSynth-DSSI is given a SoundFont.
    ///
    /// The program list is read again afterwards: FluidSynth-DSSI has **no**
    /// programs until it is given a SoundFont, and every program it then has
    /// came out of that file.
    pub fn configure(&mut self, key: &str, value: &str) -> Option<String> {
        let msg = self.inst.configure(key, value);
        // SAFETY: the instance is live and still ours — nothing has moved it to
        // the audio thread yet, which is the only place that would race.
        self.inst.programs = unsafe { read_programs(&self.inst) };
        msg
    }
}

impl AudioSource for DssiInstrument {
    fn presets(&self) -> Option<choz_ports::PresetsHandle> {
        if self.inst.programs.is_empty() {
            return None;
        }
        Some(Arc::new(DssiPresets {
            programs: self.inst.programs.clone(),
            request: Arc::clone(&self.inst.program_request),
        }) as choz_ports::PresetsHandle)
    }

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
        self.inst
            .queue_midi([0xE0, (v & 0x7F) as u8, (v >> 7) as u8]);
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

/// Libraries `dlopen`ed so far, held for the life of the process.
///
/// A plugin binary is not safe to unload and load again. FluidSynth-DSSI drags
/// in libinstpatch, which registers GLib types on load; the second time round
/// GLib says `cannot register existing type 'IpatchConverter'` and the process
/// hangs. Rui's Qt plugins crash in `_dl_close` for a neighbouring reason, which
/// is why `choz-plugin-lv2` grew the same list first. Real hosts don't unload
/// plugin binaries either.
///
/// ponytail: bounded by how many distinct plugins one session touches, and the
/// instances themselves are still cleaned up properly.
static LOADED_LIBS: std::sync::Mutex<Vec<Arc<Library>>> = std::sync::Mutex::new(Vec::new());

fn keep_loaded(lib: &Arc<Library>) {
    LOADED_LIBS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Arc::clone(lib));
}

// `free(3)`, for the strings DSSI plugins hand back from `configure`. Declared
// here rather than pulling in `libc` for one symbol: it is in the C runtime
// every plugin is already linked against.
unsafe extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut std::os::raw::c_void);
}
