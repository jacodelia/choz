//! VST2 plugin hosting for choz.
//!
//! Loads a plugin's `VSTPluginMain`, drives `processReplacing` for audio and
//! `effProcessEvents` for MIDI — the same publicly documented binary interface
//! every DAW uses, no Steinberg SDK involved.
//!
//! ```text
//! choz-plugin-vst2
//!   ├── scan_directory(dir) ← dlopen + AEffect metadata → Vst2PluginInfo
//!   ├── Vst2Effect          ← choz_ports::FxProcessor (audio → audio)
//!   └── Vst2Instrument      ← choz_ports::AudioSource (notes → audio)
//! ```
//!
//! Ported from seqterm's `seqterm-plugin-vst2` (host/registry layer dropped).

pub mod vst2_abi;

use std::os::raw::{c_char, c_float, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libloading::Library;

use choz_ports::{AudioSource, EditorHandle, FxProcessor, PluginEditor, PluginParam};
use vst2_abi::*;

/// Cap on MIDI events queued between two blocks (RT-safe: drop when full).
const MAX_PENDING_MIDI: usize = 256;

/// A discovered VST2 plugin.
#[derive(Debug, Clone)]
pub struct Vst2PluginInfo {
    pub path: PathBuf,
    pub name: String,
    pub vendor: String,
    /// `vst2:<unique id in hex>` — stable across scans, unlike the file name.
    pub id: String,
    pub is_instrument: bool,
}

/// Host callback. VST2 requires a plain function pointer, so the answers here
/// are static; the real sample rate and block size are pushed to the plugin
/// with `effSetSampleRate`/`effSetBlockSize` right after loading.
unsafe extern "C" fn host_callback(
    _effect: AEffectPtr,
    opcode: i32,
    _index: i32,
    _value: isize,
    _ptr: *mut c_void,
    _opt: c_float,
) -> isize {
    match opcode {
        host_opcode::VERSION => VST_VERSION as isize,
        host_opcode::GET_SAMPLE_RATE => 48_000,
        host_opcode::GET_BLOCK_SIZE => 512,
        host_opcode::WANT_MIDI => 1,
        // Plugins ask for this every block and many (u-he's, for one) read the
        // answer without a null check — returning 0 here is a segfault.
        host_opcode::GET_TIME => TIME_INFO.with(|t| t.get() as isize),
        // kVstProcessLevelRealtime: this callback only ever runs from `run`.
        host_opcode::PROCESS_LEVEL => 2,
        _ => 0,
    }
}

thread_local! {
    /// The transport handed back on `audioMasterGetTime`, one per calling
    /// thread so the pointer stays valid without sharing it across threads.
    ///
    /// ponytail: a fixed 120 BPM at bar one, because choz has no transport of
    /// its own. Fill it from the real clock when choz grows one — tempo-synced
    /// delays and arpeggiators will follow it then.
    static TIME_INFO: std::cell::UnsafeCell<VstTimeInfo> =
        std::cell::UnsafeCell::new(VstTimeInfo {
            sample_rate: 48_000.0,
            tempo: 120.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            flags: time_flags::TRANSPORT_PLAYING
                | time_flags::PPQ_POS_VALID
                | time_flags::TEMPO_VALID
                | time_flags::TIME_SIG_VALID,
            ..VstTimeInfo::default()
        });
}

// ─── Discovery ──────────────────────────────────────────────────────────────

/// Every VST2 plugin under `dir`. Files that aren't VST2 are skipped silently —
/// plugin directories are full of unrelated shared libraries.
pub fn scan_directory(dir: &Path) -> Vec<Vst2PluginInfo> {
    let mut out = Vec::new();
    scan_recursive(dir, 0, &mut out);
    out
}

fn scan_recursive(dir: &Path, depth: usize, out: &mut Vec<Vst2PluginInfo>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            scan_recursive(&path, depth + 1, out);
        } else if path.extension().is_some_and(|e| {
            let e = e.to_string_lossy().to_lowercase();
            e == "so" || e == "dll" || e == "vst" || e == "dylib"
        }) {
            out.extend(describe(&path));
        }
    }
}

/// Carla's own VST wrapper, which is a plugin *host*. It does not merely crash:
/// it corrupts the allocator (`free(): corrupted unsorted chunks`), so it is
/// skipped by name rather than tried.
///
/// The general case is handled by `choz_engine::quarantine`, which probes every
/// unknown plugin in a child process. This one stays hardcoded because heap
/// corruption is worth not reproducing even once, and hosting a host inside
/// choz makes no sense anyway.
fn is_denied(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_lowercase().starts_with("carla"))
}

/// Read one plugin's metadata, or `None` when the file isn't a VST2 plugin.
pub fn describe(path: &Path) -> Option<Vst2PluginInfo> {
    if is_denied(path) {
        return None;
    }
    let inst = Instance::load(path, 48_000, 64).ok()?;
    let name = inst.get_string(opcode::GET_PLUGIN_NAME, 0);
    Some(Vst2PluginInfo {
        path: path.to_path_buf(),
        name: if name.is_empty() {
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            name
        },
        vendor: inst.get_string(opcode::GET_VENDOR_STRING, 0),
        id: format!("vst2:{:08x}", unsafe { (*inst.effect).unique_id }),
        is_instrument: inst.is_synth(),
    })
}

/// The plugin's parameters. VST2 parameters are always normalised 0..1, so the
/// bounds are fixed and only the names come from the plugin.
pub fn read_params(path: &Path, _id: &str) -> Vec<PluginParam> {
    match Instance::load(path, 48_000, 64) {
        Ok(inst) => inst.params(),
        Err(_) => Vec::new(),
    }
}

// ─── Instance ───────────────────────────────────────────────────────────────

/// A loaded plugin: the `AEffect`, its de-interleaved port buffers, and the
/// library kept alive under it.
struct Instance {
    effect: AEffectPtr,
    _lib: Arc<Library>,
    block_size: usize,
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    pending_midi: Vec<[u8; 3]>,
    /// Event structs handed to the plugin; pre-allocated, reused every block.
    events: Vec<VstMidiEvent>,
    /// The `VstEvents` block: header + one pointer per event. Pre-allocated.
    event_block: Vec<u8>,
    opened: bool,
    /// The same `AEffect`, reachable from the GUI thread for editor opcodes.
    shared: SharedEffect,
}

/// The `AEffect` shared with the editor's GUI thread. Set to `None` by the
/// instance's `Drop` before the plugin is closed, which is what lets an editor
/// handle safely outlive the plugin: past that point every call is a no-op.
type SharedEffect = Arc<std::sync::Mutex<Option<EffectCell>>>;

struct EffectCell {
    effect: AEffectPtr,
    /// Keeps the shared object mapped while the GUI thread can still call in.
    _lib: Arc<Library>,
}

// SAFETY: the pointer is only read under the mutex, by the GUI thread (editor
// opcodes) or by the owning instance's Drop.
unsafe impl Send for EffectCell {}

/// A plugin's native window, driven from the UI's editor thread.
///
/// VST2 hosts have always called `effEditIdle` from the GUI thread while
/// `processReplacing` runs on the audio thread; plugins expect exactly that.
/// The mutex here is not about that split — it only guards the *lifetime*, so a
/// window that is still open when its slot is replaced stops calling a freed
/// `AEffect`.
struct Vst2Editor {
    shared: SharedEffect,
}

impl Vst2Editor {
    fn dispatch(&self, opcode: i32, value: isize, ptr: *mut c_void) -> isize {
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return 0 };
        // SAFETY: the cell is `Some` only while the instance owning this
        // `AEffect` is alive.
        unsafe {
            match (*cell.effect).dispatcher {
                Some(d) => d(cell.effect, opcode, 0, value, ptr, 0.0),
                None => 0,
            }
        }
    }

    /// `effEditGetRect` → (width, height).
    fn rect(&self) -> Option<(u16, u16)> {
        let mut rect_ptr: *mut ERect = std::ptr::null_mut();
        self.dispatch(
            opcode::EDIT_GET_RECT,
            0,
            &mut rect_ptr as *mut *mut ERect as *mut c_void,
        );
        if rect_ptr.is_null() {
            return None;
        }
        // SAFETY: the plugin wrote a valid ERect pointer, owned by the plugin.
        let r = unsafe { *rect_ptr };
        let w = (r.right - r.left).max(0) as u16;
        let h = (r.bottom - r.top).max(0) as u16;
        (w > 0 && h > 0).then_some((w, h))
    }
}

impl PluginEditor for Vst2Editor {
    fn open(&self, parent: u64) -> Option<(u16, u16)> {
        self.dispatch(opcode::EDIT_OPEN, 0, parent as usize as *mut c_void);
        self.rect()
    }

    fn idle(&self) {
        self.dispatch(opcode::EDIT_IDLE, 0, std::ptr::null_mut());
    }

    fn close(&self) {
        self.dispatch(opcode::EDIT_CLOSE, 0, std::ptr::null_mut());
    }
}

// SAFETY: an Instance owns its AEffect exclusively — built on the UI thread,
// then moved to the audio thread and never shared.
unsafe impl Send for Instance {}

/// Byte offset of the pointer array inside a `VstEvents` block:
/// `[i32 numEvents][pad][isize reserved][ptr…]`.
const EVENT_ARRAY_OFFSET: usize = 8 + std::mem::size_of::<isize>();

impl Instance {
    fn load(path: &Path, sample_rate: u32, block_size: u32) -> Result<Self> {
        if is_denied(path) {
            bail!("{} is a plugin host itself; choz will not load it", path.display());
        }
        let lib = Arc::new(
            unsafe { Library::new(path) }.with_context(|| format!("dlopen {}", path.display()))?,
        );
        let effect = unsafe {
            let main: libloading::Symbol<VstPluginMainFn> = lib
                .get(b"VSTPluginMain\0")
                .or_else(|_| lib.get(b"main\0"))
                .context("no VSTPluginMain entry point")?;
            main(host_callback)
        };
        if effect.is_null() {
            bail!("VSTPluginMain returned null for {}", path.display());
        }
        if unsafe { (*effect).magic } != VST_MAGIC {
            bail!("{} is not a VST2 plugin", path.display());
        }

        let block = block_size as usize;
        let n_in = unsafe { (*effect).num_inputs }.max(0) as usize;
        let n_out = unsafe { (*effect).num_outputs }.max(0) as usize;
        let mut inst = Self {
            effect,
            _lib: Arc::clone(&lib),
            block_size: block,
            inputs: vec![vec![0.0; block]; n_in.max(1)],
            outputs: vec![vec![0.0; block]; n_out.max(1)],
            pending_midi: Vec::with_capacity(MAX_PENDING_MIDI),
            events: Vec::with_capacity(MAX_PENDING_MIDI),
            event_block: vec![
                0u8;
                EVENT_ARRAY_OFFSET + MAX_PENDING_MIDI * std::mem::size_of::<*mut VstMidiEvent>()
            ],
            opened: false,
            shared: Arc::new(std::sync::Mutex::new(Some(EffectCell {
                effect,
                _lib: Arc::clone(&lib),
            }))),
        };
        inst.dispatch(opcode::OPEN, 0, 0, std::ptr::null_mut(), 0.0);
        inst.opened = true;
        inst.dispatch(opcode::SET_SAMPLE_RATE, 0, 0, std::ptr::null_mut(), sample_rate as f32);
        inst.dispatch(opcode::SET_BLOCK_SIZE, 0, block as isize, std::ptr::null_mut(), 0.0);
        inst.dispatch(opcode::MAIN_RESUME, 0, 1, std::ptr::null_mut(), 0.0);
        Ok(inst)
    }

    fn dispatch(&self, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
        // SAFETY: the dispatcher belongs to the AEffect this instance owns.
        unsafe {
            match (*self.effect).dispatcher {
                Some(d) => d(self.effect, opcode, index, value, ptr, opt),
                None => 0,
            }
        }
    }

    fn is_synth(&self) -> bool {
        unsafe { (*self.effect).flags & flags::IS_SYNTH != 0 }
    }

    /// Handle to the plugin's own window, or `None` if it has no editor.
    fn editor(&self) -> Option<EditorHandle> {
        if unsafe { (*self.effect).flags & flags::HAS_EDITOR } == 0 {
            return None;
        }
        Some(Arc::new(Vst2Editor { shared: Arc::clone(&self.shared) }))
    }

    fn num_params(&self) -> usize {
        unsafe { (*self.effect).num_params }.max(0) as usize
    }

    /// A NUL-terminated string from a dispatcher opcode (names, labels…).
    fn get_string(&self, op: i32, index: i32) -> String {
        let mut buf = [0 as c_char; 256];
        self.dispatch(op, index, 0, buf.as_mut_ptr() as *mut c_void, 0.0);
        c_str_from_buf(&buf)
    }

    fn params(&self) -> Vec<PluginParam> {
        (0..self.num_params())
            .map(|i| PluginParam {
                id: i as u32,
                name: {
                    let n = self.get_string(opcode::GET_PARAM_NAME, i as i32);
                    if n.is_empty() { format!("P{i}") } else { n }
                },
                // VST2 parameters are normalised by definition.
                min: 0.0,
                max: 1.0,
                default: self.get_param(i) as f64,
            })
            .collect()
    }

    fn get_param(&self, index: usize) -> f32 {
        unsafe {
            match (*self.effect).get_parameter {
                Some(f) => f(self.effect, index as i32),
                None => 0.0,
            }
        }
    }

    /// RT-safe: a direct call into the plugin, no allocation.
    fn set_param(&mut self, index: usize, value: f32) {
        if index >= self.num_params() {
            return;
        }
        unsafe {
            if let Some(f) = (*self.effect).set_parameter {
                f(self.effect, index as i32, value.clamp(0.0, 1.0));
            }
        }
    }

    fn queue_midi(&mut self, data: [u8; 3]) {
        if self.pending_midi.len() < MAX_PENDING_MIDI {
            self.pending_midi.push(data);
        }
    }

    /// Hand the queued MIDI to the plugin for the coming block. RT-safe: both
    /// the event structs and the pointer block were allocated up front.
    fn send_pending_midi(&mut self) {
        if self.pending_midi.is_empty() {
            return;
        }
        self.events.clear();
        for m in &self.pending_midi {
            self.events.push(VstMidiEvent {
                event_type: VST_MIDI_TYPE,
                byte_size: std::mem::size_of::<VstMidiEvent>() as i32,
                delta_frames: 0,
                flags: 0,
                note_length: 0,
                note_offset: 0,
                midi_data: [m[0], m[1], m[2], 0],
                detune: 0,
                note_off_velocity: 0,
                reserved1: 0,
                reserved2: 0,
            });
        }
        self.pending_midi.clear();

        // SAFETY: `event_block` has room for MAX_PENDING_MIDI pointers, and the
        // events it points at live in `self.events` until after the dispatch.
        unsafe {
            let base = self.event_block.as_mut_ptr();
            *(base as *mut i32) = self.events.len() as i32;
            let arr = base.add(EVENT_ARRAY_OFFSET) as *mut *mut VstMidiEvent;
            for (i, ev) in self.events.iter_mut().enumerate() {
                *arr.add(i) = ev as *mut VstMidiEvent;
            }
        }
        let block = self.event_block.as_mut_ptr() as *mut c_void;
        self.dispatch(opcode::PROCESS_EVENTS, 0, 0, block, 0.0);
    }

    /// Run `frames` through `processReplacing`, with the inputs already filled.
    fn run(&mut self, frames: usize) {
        self.send_pending_midi();
        let mut in_ptrs: [*mut c_float; 32] = [std::ptr::null_mut(); 32];
        let mut out_ptrs: [*mut c_float; 32] = [std::ptr::null_mut(); 32];
        for (i, b) in self.inputs.iter_mut().take(32).enumerate() {
            in_ptrs[i] = b.as_mut_ptr();
        }
        for (i, b) in self.outputs.iter_mut().take(32).enumerate() {
            out_ptrs[i] = b.as_mut_ptr();
        }
        // SAFETY: every pointer addresses a buffer of at least `frames` samples,
        // and `frames <= block_size`, which is what the plugin was told.
        unsafe {
            if let Some(process) = (*self.effect).process_replacing {
                process(
                    self.effect,
                    in_ptrs.as_mut_ptr(),
                    out_ptrs.as_mut_ptr(),
                    frames as std::os::raw::c_int,
                );
            }
        }
    }

    /// Frame `f` of the output as a stereo pair; mono is duplicated and
    /// non-finite samples are dropped rather than passed on.
    fn out_frame(&self, f: usize) -> (f32, f32) {
        let n = unsafe { (*self.effect).num_outputs }.max(0) as usize;
        let (l, r) = match n {
            0 => (0.0, 0.0),
            1 => (self.outputs[0][f], self.outputs[0][f]),
            _ => (self.outputs[0][f], self.outputs[1][f]),
        };
        if l.is_finite() && r.is_finite() { (l, r) } else { (0.0, 0.0) }
    }

    /// One block of interleaved stereo through the plugin, in place.
    fn process_interleaved(&mut self, block: &mut [f32], wet: f32) {
        let frames = (block.len() / 2).min(self.block_size);
        let n_in = unsafe { (*self.effect).num_inputs }.max(0) as usize;
        let mono_in = n_in == 1;
        for (ch, buf) in self.inputs.iter_mut().take(n_in).enumerate() {
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

    /// Render a block as an instrument: inputs silent, output written out.
    fn render_block(&mut self, out: &mut [f32]) -> usize {
        let frames = (out.len() / 2).min(self.block_size);
        if frames == 0 {
            return 0;
        }
        for buf in self.inputs.iter_mut() {
            buf[..frames].fill(0.0);
        }
        self.run(frames);
        for f in 0..frames {
            let (l, r) = self.out_frame(f);
            out[f * 2] = l;
            out[f * 2 + 1] = r;
        }
        frames
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Cut the editor thread loose first: past this it can no longer reach
        // the AEffect we are about to close.
        if let Some(cell) = self.shared.lock().unwrap_or_else(|e| e.into_inner()).take() {
            drop(cell);
        }
        self.dispatch(opcode::EDIT_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
        self.dispatch(opcode::MAIN_RESUME, 0, 0, std::ptr::null_mut(), 0.0);
        if self.opened {
            self.dispatch(opcode::CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
        }
    }
}

// ─── Effect / instrument wrappers ───────────────────────────────────────────

/// A live VST2 audio effect in a slot's FX chain.
pub struct Vst2Effect {
    inst: Instance,
    wet: f32,
}

impl Vst2Effect {
    /// Load the VST2 effect at `path`. `None` on any failure.
    pub fn build(path: &Path, sample_rate: u32, max_block: u32) -> Option<Self> {
        match Instance::load(path, sample_rate, max_block) {
            Ok(inst) => Some(Self { inst, wet: 1.0 }),
            Err(e) => {
                eprintln!("choz: VST2 {}: {e}", path.display());
                None
            }
        }
    }
}

impl FxProcessor for Vst2Effect {
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
        (0..self.inst.num_params())
            .map(|_| choz_ports::FxParam::new("param", 0.0, 0.0, 1.0, ""))
            .collect()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param(index, value);
    }

    fn editor(&self) -> Option<EditorHandle> {
        self.inst.editor()
    }
}

/// A live VST2 instrument in a rack slot: notes in, interleaved stereo out.
pub struct Vst2Instrument {
    inst: Instance,
}

impl Vst2Instrument {
    /// Load the VST2 synth at `path`. `None` on any failure, including a plugin
    /// that doesn't declare itself a synth.
    pub fn build(path: &Path, sample_rate: u32, max_block: u32) -> Option<Self> {
        match Instance::load(path, sample_rate, max_block) {
            Ok(inst) if !inst.is_synth() => {
                eprintln!("choz: VST2 {} is not a synth", path.display());
                None
            }
            Ok(inst) => Some(Self { inst }),
            Err(e) => {
                eprintln!("choz: VST2 {}: {e}", path.display());
                None
            }
        }
    }
}

impl AudioSource for Vst2Instrument {
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

    fn program_change(&mut self, _bank: u8, preset: u8) {
        self.inst.dispatch(opcode::SET_PROGRAM, 0, preset as isize, std::ptr::null_mut(), 0.0);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inst.set_param(index, value);
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }

    fn editor(&self) -> Option<EditorHandle> {
        self.inst.editor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `audioMasterGetTime` must hand back a real `VstTimeInfo`. Plugins ask
    /// for it inside `processReplacing` and several (u-he's TyrellN6, found the
    /// hard way) dereference it without a null check — a 0 here is a segfault
    /// on the first block, not a missing feature.
    #[test]
    fn the_host_callback_answers_get_time_with_a_filled_in_transport() {
        let ptr = unsafe {
            host_callback(
                std::ptr::null_mut(),
                host_opcode::GET_TIME,
                0,
                0,
                std::ptr::null_mut(),
                0.0,
            )
        };
        assert_ne!(ptr, 0, "audioMasterGetTime returned null");
        let info = unsafe { *(ptr as *const VstTimeInfo) };
        assert!(info.sample_rate > 0.0);
        assert!(info.tempo > 0.0);
        assert_eq!(info.flags & time_flags::TEMPO_VALID, time_flags::TEMPO_VALID);
    }
}
