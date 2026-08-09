//! LV2 plugin hosting for choz.
//!
//! Parses LV2 bundle TTL (Turtle/RDF) with a pure-Rust parser and loads the
//! plugin shared library via `libloading`, driving the LV2 C ABI directly — no
//! `liblilv`, no LV2 SDK.
//!
//! ```text
//! choz-plugin-lv2
//!   ├── scan_directory(dir)  ← parses *.lv2 bundle TTL → Lv2PluginInfo
//!   ├── Lv2Instance          ← raw handle + Library kept alive + port buffers
//!   ├── Lv2Instrument        ← choz_ports::AudioSource (notes → audio)
//!   └── Lv2Effect            ← choz_ports::FxProcessor (audio → audio)
//! ```
//!
//! Ported from seqterm's `seqterm-plugin-lv2` (host/registry layer dropped:
//! choz builds one self-contained instance per rack slot).

pub mod discovery;
pub mod state;
pub mod editor;
pub mod lv2_abi;
pub mod ttl;

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libloading::Library;
use parking_lot::Mutex;

use choz_ports::{AudioSource, FxProcessor, PluginParam};

pub use discovery::{Lv2PluginInfo, allow_denied_uis};
use discovery::{Port, PortKind};
use lv2_abi::*;

// ─── URID map (shared across instances of one host) ─────────────────────────

/// Backing store for the `urid:map`/`urid:unmap` host features. Assigns a stable
/// integer to each URI and keeps the C strings alive for `unmap`.
pub(crate) struct UridStore {
    map: HashMap<String, u32>,
    names: Vec<CString>, // names[urid - 1]
    next: u32,
}

impl UridStore {
    /// The URI a URID stands for, if this store minted it. State has to be
    /// written down as URIs: the numbers only mean something inside one run.
    fn uri(&self, urid: u32) -> Option<String> {
        self.names
            .get(urid.checked_sub(1)? as usize)
            .map(|c| c.to_string_lossy().into_owned())
    }

    fn new() -> Self {
        Self { map: HashMap::new(), names: Vec::new(), next: 1 }
    }
    fn intern(&mut self, uri: &str) -> u32 {
        if let Some(&id) = self.map.get(uri) {
            return id;
        }
        let id = self.next;
        self.next += 1;
        self.map.insert(uri.to_string(), id);
        self.names.push(CString::new(uri).unwrap_or_default());
        id
    }
}

unsafe extern "C" fn urid_map_fn(handle: LV2_URID_Map_Handle, uri: *const c_char) -> LV2_URID {
    if uri.is_null() || handle.is_null() {
        return 0;
    }
    let store = unsafe { &*(handle as *const Mutex<UridStore>) };
    let s = unsafe { CStr::from_ptr(uri) }.to_string_lossy().into_owned();
    store.lock().intern(&s)
}

/// One URID store shared by every plugin *UI*, kept for the process's life.
///
/// A UI only maps URIs for its own bookkeeping — choz exchanges plain floats
/// with it, never atoms — so it does not need to agree with the DSP instance's
/// numbering. A `static` keeps the handle valid for as long as any window can
/// still call in, which a per-instance store would not.
static UI_URIDS: std::sync::OnceLock<Arc<Mutex<UridStore>>> = std::sync::OnceLock::new();

/// An `LV2_URID_Map` over [`UI_URIDS`], for a UI's feature array.
pub(crate) fn shared_urid_map() -> LV2_URID_Map {
    let store = UI_URIDS.get_or_init(|| Arc::new(Mutex::new(UridStore::new())));
    LV2_URID_Map {
        handle: Arc::as_ptr(store) as *mut c_void,
        map: Some(urid_map_fn),
    }
}

unsafe extern "C" fn urid_unmap_fn(handle: LV2_URID_Unmap_Handle, urid: LV2_URID) -> *const c_char {
    if handle.is_null() || urid == 0 {
        return std::ptr::null();
    }
    let store = unsafe { &*(handle as *const Mutex<UridStore>) };
    let guard = store.lock();
    match guard.names.get((urid - 1) as usize) {
        Some(cs) => cs.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Pre-built host features (`urid:map`, `urid:unmap`, `opts:options`) for one
/// instance. The boxed structs and the null-terminated pointer array must
/// outlive the plugin.
struct Features {
    _store: Arc<Mutex<UridStore>>,
    _map: Box<LV2_URID_Map>,
    _unmap: Box<LV2_URID_Unmap>,
    _uris: Vec<CString>,
    /// Option values the option array points at; the Vec's buffer never moves.
    _opt_values: Vec<OptValue>,
    /// Null-terminated array of options handed to the plugin.
    _options: Vec<LV2_Options_Option>,
    _feats: Vec<LV2_Feature>,
    /// Null-terminated array of `*const LV2_Feature` passed to `instantiate`.
    ptrs: Vec<*const LV2_Feature>,
    /// Worker plumbing, filled in once the plugin handle exists.
    worker: Box<WorkerState>,
    _schedule: Box<LV2_Worker_Schedule>,
    /// The URID assigned to `midi:MidiEvent` (for building Atom sequences).
    midi_urid: u32,
    /// The URID assigned to `atom:Sequence` (the MIDI port's atom type).
    sequence_urid: u32,
}

/// One `opts:options` value: an `atom:Int` or an `atom:Float`.
enum OptValue {
    Int(i32),
    Float(f32),
}

impl OptValue {
    fn ptr(&self) -> *const c_void {
        match self {
            OptValue::Int(v) => v as *const i32 as *const c_void,
            OptValue::Float(v) => v as *const f32 as *const c_void,
        }
    }
    fn size(&self) -> u32 {
        4
    }
}

/// Wiring for the `worker#schedule` feature.
///
/// The plugin is handed the schedule callback at instantiate time, before its
/// own handle exists, so the handle and its worker interface are filled in
/// afterwards — the plugin can only call back from `run()`, which is long after
/// that.
///
/// The work itself runs **synchronously, on the audio thread**, but the answer
/// is not: `respond` only queues it, and `run` hands it to `work_response`
/// after `work` has returned. That ordering is what the spec describes, and
/// plugins that keep their own worker thread (Rui's `*v1` family) crash if
/// `work_response` is re-entered from inside `work`.
///
/// ponytail: doing the work inline is what makes these plugins load at all
/// instead of being rejected outright. If one of them stalls the audio thread
/// (a big sample load), move `work()` onto a thread with a request ring and
/// deliver the response at the top of the next `run`.
struct WorkerState {
    handle: std::cell::Cell<LV2_Handle>,
    iface: std::cell::Cell<*const LV2_Worker_Interface>,
    /// Answers queued by `respond`, drained by `run`.
    responses: std::cell::RefCell<Vec<Vec<u8>>>,
}

impl WorkerState {
    fn new() -> Box<Self> {
        Box::new(Self {
            handle: std::cell::Cell::new(std::ptr::null_mut()),
            iface: std::cell::Cell::new(std::ptr::null()),
            responses: std::cell::RefCell::new(Vec::new()),
        })
    }
}

/// `LV2_Worker_Schedule.schedule_work`: run the job now and answer immediately.
unsafe extern "C" fn worker_schedule_fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    if handle.is_null() {
        return LV2_WORKER_ERR_UNKNOWN;
    }
    // SAFETY: `handle` is the `WorkerState` box owned by this instance's
    // `Features`, which outlives the plugin.
    let state = unsafe { &*(handle as *const WorkerState) };
    let (inst, iface) = (state.handle.get(), state.iface.get());
    if inst.is_null() || iface.is_null() {
        return LV2_WORKER_ERR_UNKNOWN;
    }
    match unsafe { (*iface).work } {
        Some(work) => unsafe { work(inst, Some(worker_respond_fn), handle, size, data) },
        None => LV2_WORKER_ERR_UNKNOWN,
    }
}

/// The `respond` callback handed to `work()`: copy the answer aside. It reaches
/// the plugin from [`Lv2Instance::run`], never from inside `work` itself.
unsafe extern "C" fn worker_respond_fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    if handle.is_null() {
        return LV2_WORKER_ERR_UNKNOWN;
    }
    // SAFETY: same box as above — `work()` was given this very pointer.
    let state = unsafe { &*(handle as *const WorkerState) };
    let body = if data.is_null() || size == 0 {
        Vec::new()
    } else {
        // SAFETY: the plugin promises `size` readable bytes at `data`; they are
        // only valid for the duration of this call, hence the copy.
        unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) }.to_vec()
    };
    match state.responses.try_borrow_mut() {
        Ok(mut q) => {
            q.push(body);
            LV2_WORKER_SUCCESS
        }
        Err(_) => LV2_WORKER_ERR_UNKNOWN,
    }
}

impl Features {
    /// `block_size` is the largest block the plugin will ever be given, and the
    /// value reported as `bufsz:maxBlockLength` — DPF-based plugins allocate
    /// from it, so it must match what `run()` actually gets.
    fn new(store: Arc<Mutex<UridStore>>, sample_rate: u32, block_size: u32) -> Self {
        let store_ptr = Arc::as_ptr(&store) as *mut c_void;
        let mut map = Box::new(LV2_URID_Map { handle: store_ptr, map: Some(urid_map_fn) });
        let mut unmap = Box::new(LV2_URID_Unmap { handle: store_ptr, unmap: Some(urid_unmap_fn) });
        let (midi_urid, sequence_urid, int_urid, float_urid, opt_keys) = {
            let mut s = store.lock();
            (
                s.intern(LV2_MIDI_EVENT_URI),
                s.intern(LV2_ATOM_SEQUENCE_URI),
                s.intern(LV2_ATOM_INT_URI),
                s.intern(LV2_ATOM_FLOAT_URI),
                [
                    s.intern(LV2_BUF_SIZE_MIN_BLOCK_URI),
                    s.intern(LV2_BUF_SIZE_MAX_BLOCK_URI),
                    s.intern(LV2_BUF_SIZE_NOMINAL_BLOCK_URI),
                    s.intern(LV2_PARAM_SAMPLE_RATE_URI),
                ],
            )
        };

        // Values first: the options array points into these boxes.
        let opt_values: Vec<OptValue> = vec![
            OptValue::Int(1),
            OptValue::Int(block_size as i32),
            OptValue::Int(block_size as i32),
            OptValue::Float(sample_rate as f32),
        ];
        let mut options: Vec<LV2_Options_Option> = opt_values
            .iter()
            .zip(opt_keys)
            .map(|(v, key)| LV2_Options_Option {
                context: LV2_OPTIONS_INSTANCE,
                subject: 0,
                key,
                size: v.size(),
                type_: match *v {
                    OptValue::Int(_) => int_urid,
                    OptValue::Float(_) => float_urid,
                },
                value: v.ptr(),
            })
            .collect();
        // Terminator.
        options.push(LV2_Options_Option {
            context: 0,
            subject: 0,
            key: 0,
            size: 0,
            type_: 0,
            value: std::ptr::null(),
        });

        let uris: Vec<CString> =
            [LV2_URID_MAP_URI, LV2_URID_UNMAP_URI, LV2_OPTIONS_URI, LV2_WORKER_SCHEDULE_URI]
                .iter()
                .map(|u| CString::new(*u).unwrap_or_default())
                .collect();
        let worker = WorkerState::new();
        let mut schedule = Box::new(LV2_Worker_Schedule {
            handle: worker.as_ref() as *const WorkerState as *mut c_void,
            schedule_work: Some(worker_schedule_fn),
        });
        let feats = vec![
            LV2_Feature {
                uri: uris[0].as_ptr(),
                data: map.as_mut() as *mut LV2_URID_Map as *mut c_void,
            },
            LV2_Feature {
                uri: uris[1].as_ptr(),
                data: unmap.as_mut() as *mut LV2_URID_Unmap as *mut c_void,
            },
            LV2_Feature {
                uri: uris[2].as_ptr(),
                data: options.as_ptr() as *mut c_void,
            },
            LV2_Feature {
                uri: uris[3].as_ptr(),
                data: schedule.as_mut() as *mut LV2_Worker_Schedule as *mut c_void,
            },
        ];
        // Pointers into the Vec's heap buffer, which stays put when `feats`
        // (the Vec header) is moved into `Self` below.
        let ptrs: Vec<*const LV2_Feature> = feats
            .iter()
            .map(|f| f as *const LV2_Feature)
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        Self {
            _store: store,
            _map: map,
            _unmap: unmap,
            _uris: uris,
            _opt_values: opt_values,
            _options: options,
            _feats: feats,
            ptrs,
            worker,
            _schedule: schedule,
            midi_urid,
            sequence_urid,
        }
    }

    /// URIs of features we provide; anything else in `requiredFeature` is fatal.
    /// `boundedBlockLength` holds because every block is chunked to the size
    /// reported in the options above.
    fn supported(uri: &str) -> bool {
        // The two path features are not passed here — they belong to `save` and
        // `restore`, which do provide them — but a plugin that *requires* them
        // is asking whether this host can store its file paths at all, and it
        // can.
        matches!(uri, LV2_URID_MAP_URI | LV2_URID_UNMAP_URI | LV2_OPTIONS_URI
            | LV2_BUF_SIZE_BOUNDED_URI | LV2_WORKER_SCHEDULE_URI
            | LV2_STATE_MAP_PATH_URI | LV2_STATE_FREE_PATH_URI)
    }
}

// ─── Instance ───────────────────────────────────────────────────────────────

struct Lv2Instance {
    handle: LV2_Handle,
    descriptor: *const LV2_Descriptor,
    _lib: Arc<Library>,
    features: Features,
    info: Lv2PluginInfo,
    block_size: usize,

    /// One f32 cell per port index (control + fallback for unused ports).
    control_values: Vec<f32>,
    /// Per-port audio buffer (only audio ports are non-empty), indexed by port idx.
    audio_bufs: Vec<Vec<f32>>,
    /// Per-port atom byte buffer (only atom ports are non-empty), indexed by port idx.
    atom_bufs: Vec<Vec<u8>>,

    audio_in: Vec<usize>,   // port indices
    audio_out: Vec<usize>,  // port indices
    atom_in: Option<usize>, // first MIDI atom input port index
    atom_out: Vec<usize>,   // atom output port indices (need capacity reset)
    /// MIDI queued via `send_midi`, drained into the atom sequence each `process`.
    pending_midi: Vec<[u8; 3]>,
    activated: bool,
    /// Handed to the plugin's own window so it can move control ports. Emptied
    /// in `Drop`, so a window still open when the slot goes away stops writing.
    controls: editor::SharedControls,
    /// The instance, for saving and restoring its own state. Emptied in `Drop`
    /// alongside the controls.
    state: state::SharedState,
}

// SAFETY: raw pointers into the loaded library are only touched while the
// owning `Lv2PluginHost` holds its `Mutex`, serialising all access.
unsafe impl Send for Lv2Instance {}
unsafe impl Sync for Lv2Instance {}

const ATOM_BUF_BYTES: usize = 8192;
/// Cap on MIDI events queued between `process` calls (RT-safe: drop when full).
const MAX_PENDING_MIDI: usize = 256;

impl Lv2Instance {
    fn connect_all(&mut self) {
        let Some(connect) = (unsafe { (*self.descriptor).connect_port }) else { return };
        let nports = self.control_values.len();
        for i in 0..nports {
            let ptr: *mut c_void = if !self.audio_bufs[i].is_empty() {
                self.audio_bufs[i].as_mut_ptr() as *mut c_void
            } else if !self.atom_bufs[i].is_empty() {
                self.atom_bufs[i].as_mut_ptr() as *mut c_void
            } else {
                (&mut self.control_values[i]) as *mut f32 as *mut c_void
            };
            unsafe { connect(self.handle, i as u32, ptr) };
        }
    }

    /// Write the queued MIDI as an `LV2_Atom_Sequence` into the MIDI input port.
    fn write_midi_sequence(&mut self) {
        let Some(idx) = self.atom_in else { return };
        let midi_urid = self.features.midi_urid;
        let buf = &mut self.atom_bufs[idx];
        if buf.len() < std::mem::size_of::<LV2_Atom_Sequence>() {
            return;
        }
        // Sequence header: atom.size will be filled after writing events.
        let mut write = std::mem::size_of::<LV2_Atom_Sequence>();
        for msg in &self.pending_midi {
            let ev_hdr = std::mem::size_of::<LV2_Atom_Event>();
            let needed = pad8(ev_hdr + 3);
            if write + needed > buf.len() {
                break;
            }
            let ev = LV2_Atom_Event {
                frames: 0,
                body: LV2_Atom { size: 3, type_: midi_urid },
            };
            // Copy event header.
            let ev_bytes = unsafe {
                std::slice::from_raw_parts(&ev as *const _ as *const u8, ev_hdr)
            };
            buf[write..write + ev_hdr].copy_from_slice(ev_bytes);
            // Copy 3 MIDI bytes after the header.
            buf[write + ev_hdr..write + ev_hdr + 3].copy_from_slice(&msg[..3]);
            write += needed;
        }
        let body_size = (write - std::mem::size_of::<LV2_Atom>()) as u32;
        let seq = LV2_Atom_Sequence {
            atom: LV2_Atom { size: body_size, type_: self.features.sequence_urid },
            body: LV2_Atom_Sequence_Body { unit: 0, pad: 0 },
        };
        let seq_bytes = unsafe {
            std::slice::from_raw_parts(&seq as *const _ as *const u8, std::mem::size_of::<LV2_Atom_Sequence>())
        };
        buf[..seq_bytes.len()].copy_from_slice(seq_bytes);
        self.pending_midi.clear();
    }

    /// Reset atom OUTPUT ports to an empty Chunk with full available capacity,
    /// as required before `run()`.
    fn reset_atom_outputs(&mut self) {
        for &idx in &self.atom_out {
            let buf = &mut self.atom_bufs[idx];
            if buf.len() < std::mem::size_of::<LV2_Atom>() {
                continue;
            }
            let cap = (buf.len() - std::mem::size_of::<LV2_Atom>()) as u32;
            let atom = LV2_Atom { size: cap, type_: 0 };
            let bytes = unsafe {
                std::slice::from_raw_parts(&atom as *const _ as *const u8, std::mem::size_of::<LV2_Atom>())
            };
            buf[..bytes.len()].copy_from_slice(bytes);
        }
    }

    fn run(&mut self, frames: usize) {
        if let Some(run) = unsafe { (*self.descriptor).run } {
            unsafe { run(self.handle, frames as u32) };
        }
        self.deliver_worker_responses();
    }

    /// Hand the plugin whatever its `work()` answered, then close the cycle with
    /// `end_run` — both from the thread that called `run`, as the spec wants.
    fn deliver_worker_responses(&mut self) {
        let iface = self.features.worker.iface.get();
        if iface.is_null() {
            return;
        }
        let queued: Vec<Vec<u8>> = match self.features.worker.responses.try_borrow_mut() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => return,
        };
        for body in queued {
            if let Some(respond) = unsafe { (*iface).work_response } {
                unsafe { respond(self.handle, body.len() as u32, body.as_ptr() as *const c_void) };
            }
        }
        if let Some(end_run) = unsafe { (*iface).end_run } {
            unsafe { end_run(self.handle) };
        }
    }

    /// Render up to one block as an instrument: feed queued MIDI, run, and write
    /// interleaved-stereo audio into `output`. Audio inputs (if any) are silenced.
    /// Returns frames written (`<= block_size`). RT-safe: no allocation.
    fn render_block(&mut self, output: &mut [f32]) -> usize {
        let frames = (output.len() / 2).min(self.block_size);
        if frames == 0 {
            return 0;
        }
        for &pi in &self.audio_in {
            for v in self.audio_bufs[pi].iter_mut().take(frames) {
                *v = 0.0;
            }
        }
        self.write_midi_sequence();
        self.reset_atom_outputs();
        self.run(frames);

        let n_out = self.audio_out.len();
        for f in 0..frames {
            let (l, r) = match n_out {
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
            output[f * 2] = l;
            output[f * 2 + 1] = r;
        }
        frames
    }

    /// Queue a raw 3-byte MIDI message for the next render (bounded; drops when full).
    fn queue_midi(&mut self, bytes: [u8; 3]) {
        if self.atom_in.is_some() && self.pending_midi.len() < MAX_PENDING_MIDI {
            self.pending_midi.push(bytes);
        }
    }
}

/// URIs the host has asked us not to destroy. `padthv1` is the known one: it
/// starts a Qt thread of its own and, once the plugin has processed a block,
/// that thread segfaults while `cleanup` joins it.
///
/// Nothing is hardcoded — `choz-engine` probes each plugin in a child process
/// the first time it is used and calls [`leak_on_teardown`] for the ones that
/// die on the way out.
///
/// ponytail: leaving one instance alive beats taking the whole app down when a
/// tab is removed; the library is kept mapped anyway (see [`LOADED_LIBS`]), so
/// its threads keep running valid code. `CHOZ_LV2_STRICT_TEARDOWN=1` ignores
/// the list, which is how the probe finds out in the first place.
static LEAKY_URIS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Mark `uri` as a plugin whose instances must not be destroyed.
pub fn leak_on_teardown(uri: &str) {
    let mut leaky = LEAKY_URIS.lock();
    if !leaky.iter().any(|u| u == uri) {
        leaky.push(uri.to_string());
    }
}

fn leaks_on_teardown(uri: &str) -> bool {
    if std::env::var_os("CHOZ_LV2_STRICT_TEARDOWN").is_some() {
        return false;
    }
    LEAKY_URIS.lock().iter().any(|u| u == uri)
}

impl Drop for Lv2Instance {
    fn drop(&mut self) {
        // Before anything else, and on every path out: `control_values` dies
        // with this struct, so an editor window that is still open must stop
        // reaching into it. This is also why the leak path below still runs it.
        *self.controls.lock() = None;
        *self.state.lock() = None;
        if leaks_on_teardown(&self.info.uri) {
            eprintln!("choz: leaving {} alive on purpose (it crashes in cleanup)", self.info.uri);
            return;
        }
        unsafe {
            if let Some(deactivate) = (*self.descriptor).deactivate.filter(|_| self.activated) {
                deactivate(self.handle);
            }
            if let Some(cleanup) = (*self.descriptor).cleanup {
                cleanup(self.handle);
            }
        }
    }
}

/// `dlopen` an LV2 binary into a reference-counted `Library`.
fn load_library(path: &Path) -> Result<Arc<Library>> {
    let l = unsafe {
        Library::new(path).with_context(|| format!("dlopen {}", path.display()))?
    };
    let lib = Arc::new(l);
    keep_loaded(&lib);
    Ok(lib)
}

/// Libraries `dlopen`ed so far, held for the life of the process.
///
/// An LV2 binary may keep threads of its own that outlive `cleanup` — Rui's
/// `*v1` plugins each drag in Qt, with its event, D-Bus and XCB threads — and
/// `dlclose`ing the code out from under them segfaults inside the dynamic
/// loader. (Reproduced with padthv1: load, run, drop, crash in `_dl_close`.)
/// Real hosts don't unload plugin binaries either.
///
/// ponytail: bounded by how many distinct plugins one session touches, and the
/// instances themselves are still torn down properly. Out-of-process hosting is
/// what makes this go away for good.
static LOADED_LIBS: Mutex<Vec<Arc<Library>>> = Mutex::new(Vec::new());

fn keep_loaded(lib: &Arc<Library>) {
    LOADED_LIBS.lock().push(Arc::clone(lib));
}

/// Build a fully connected, activated [`Lv2Instance`] from a parsed plugin info
/// and an already-loaded library. Shared by the host's `instantiate` and the
/// standalone [`Lv2InstrumentSource`].
fn build_instance(
    info: Lv2PluginInfo,
    lib: Arc<Library>,
    urids: Arc<Mutex<UridStore>>,
    sample_rate: u32,
    block_size: u32,
) -> Result<Lv2Instance> {
    // Refuse plugins needing features we don't provide.
    for feat in &info.required_features {
        if !Features::supported(feat) {
            bail!("LV2 plugin {} requires unsupported feature: {feat}", info.uri);
        }
    }

    // Resolve the entry point and find the descriptor matching the URI.
    let descriptor = unsafe {
        let entry: libloading::Symbol<Lv2DescriptorFn> = lib
            .get(LV2_DESCRIPTOR_SYM)
            .context("missing lv2_descriptor symbol")?;
        let mut i = 0u32;
        let mut found: *const LV2_Descriptor = std::ptr::null();
        loop {
            let d = entry(i);
            if d.is_null() {
                break;
            }
            let uri = CStr::from_ptr((*d).uri).to_string_lossy();
            if uri == info.uri {
                found = d;
                break;
            }
            i += 1;
        }
        if found.is_null() {
            bail!("descriptor URI {} not exported by binary", info.uri);
        }
        found
    };

    // Kept aside: the state blob is written in URIs, and only this store can
    // turn the plugin's URIDs back into them.
    let urid_store = Arc::clone(&urids);
    let features = Features::new(urids, sample_rate, block_size);

    // Allocate per-port buffers.
    let nports = info.ports.iter().map(|p| p.index as usize + 1).max().unwrap_or(0);
    let mut control_values = vec![0.0f32; nports];
    let mut audio_bufs = vec![Vec::<f32>::new(); nports];
    let mut atom_bufs = vec![Vec::<u8>::new(); nports];
    let (mut audio_in, mut audio_out, mut atom_out) = (Vec::new(), Vec::new(), Vec::new());
    let mut atom_in = None;

    for p in &info.ports {
        let i = p.index as usize;
        if i >= nports {
            continue;
        }
        match p.kind {
            PortKind::AudioInput => {
                audio_bufs[i] = vec![0.0; block_size as usize];
                audio_in.push(i);
            }
            PortKind::AudioOutput => {
                audio_bufs[i] = vec![0.0; block_size as usize];
                audio_out.push(i);
            }
            PortKind::ControlInput => {
                control_values[i] = p.default;
            }
            PortKind::ControlOutput | PortKind::Unknown => {
                control_values[i] = p.default;
            }
            PortKind::AtomInput => {
                atom_bufs[i] = vec![0u8; ATOM_BUF_BYTES];
                if p.is_midi && atom_in.is_none() {
                    atom_in = Some(i);
                }
            }
            PortKind::AtomOutput => {
                atom_bufs[i] = vec![0u8; ATOM_BUF_BYTES];
                atom_out.push(i);
            }
        }
    }

    // Instantiate the plugin.
    let handle = unsafe {
        let instantiate = (*descriptor)
            .instantiate
            .ok_or_else(|| anyhow::anyhow!("plugin has no instantiate fn"))?;
        let bundle = CString::new(format!("{}/", info.bundle_dir.to_string_lossy()))
            .unwrap_or_default();
        let h = instantiate(
            descriptor,
            sample_rate as f64,
            bundle.as_ptr(),
            features.ptrs.as_ptr(),
        );
        if h.is_null() {
            bail!("instantiate returned null for {}", info.uri);
        }
        h
    };

    // The plugin can only schedule work from `run()`, so filling this in after
    // instantiate is soon enough — and it's the earliest the handle exists.
    features.worker.handle.set(handle);
    if let Some(ext) = unsafe { (*descriptor).extension_data } {
        let uri = CString::new(LV2_WORKER_INTERFACE_URI).unwrap_or_default();
        let iface = unsafe { ext(uri.as_ptr()) } as *const LV2_Worker_Interface;
        features.worker.iface.set(iface);
    }

    let lib_for_state = Arc::clone(&lib);
    let mut inst = Lv2Instance {
        handle,
        descriptor,
        _lib: lib,
        features,
        info,
        block_size: block_size as usize,
        control_values,
        audio_bufs,
        atom_bufs,
        audio_in,
        audio_out,
        atom_in,
        atom_out,
        pending_midi: Vec::with_capacity(MAX_PENDING_MIDI),
        activated: false,
        controls: Arc::new(Mutex::new(None)),
        state: Arc::new(Mutex::new(Some(state::StateCell {
            handle,
            descriptor,
            urids: urid_store,
            _lib: lib_for_state,
        }))),
    };

    // After the move into `inst`: the cell records where `control_values` ended
    // up, and `connect_port` below hands the plugin pointers into that same
    // buffer, so both sides agree for the instance's whole life.
    *inst.controls.lock() = Some(editor::ControlsCell {
        values: inst.control_values.as_mut_ptr(),
        len: inst.control_values.len(),
    });

    inst.connect_all();
    if let Some(activate) = unsafe { (*descriptor).activate } {
        unsafe { activate(handle) };
    }
    inst.activated = true;
    Ok(inst)
}

/// Maximum directory depth searched for `*.lv2` bundles below a search root.
/// Bundles live at most a couple levels deep; this bounds the walk so a stray
/// symlink or deep tree can never turn a scan into a filesystem-wide crawl.
const MAX_SCAN_DEPTH: usize = 4;

/// Walk `dir` recursively, invoking `f` for every `*.lv2` bundle directory.
/// Symlinked directories are not followed and the walk is depth-bounded, so a
/// symlink cycle or a link into a large tree can never hang the scan.
fn scan_bundles(dir: &Path, f: &mut impl FnMut(&Path)) {
    scan_bundles_depth(dir, 0, f);
}

fn scan_bundles_depth(dir: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        // Use the entry's own (non-following) type so symlinked dirs are skipped.
        let is_dir = entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("lv2") {
            f(&path);
        } else if !is_pruned_dir(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
            scan_bundles_depth(&path, depth + 1, f);
        }
    }
}

/// Directory names never worth descending into when searching for `.lv2` bundles
/// — version control, build output, and dependency caches.
fn is_pruned_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".svn" | ".hg"
            | "target" | "build" | "node_modules"
            | ".cargo" | ".rustup" | ".cache" | "__pycache__"
    )
}

/// Platform-default LV2 search directories (Carla-style).
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut p = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        p.push(home.join(".lv2"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        p.push(PathBuf::from("/usr/lib/lv2"));
        p.push(PathBuf::from("/usr/local/lib/lv2"));
        p.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/lv2"));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            p.push(home.join("Library/Audio/Plug-Ins/LV2"));
        }
        p.push(PathBuf::from("/Library/Audio/Plug-Ins/LV2"));
    }
    let _ = &mut p;
    p
}


// ─── choz-facing API ────────────────────────────────────────────────────────

/// Every LV2 plugin under `dir` (bundles are `*.lv2` directories). Pure TTL
/// parsing — no library is loaded, so a scan is cheap and can't crash us.
pub fn scan_directory(dir: &Path) -> Vec<Lv2PluginInfo> {
    let mut out = Vec::new();
    scan_bundles(dir, &mut |bundle| out.extend(discovery::discover_bundle(bundle)));
    out
}

/// The plugin `uri` inside the bundle at `bundle_dir`.
fn info_for(bundle_dir: &Path, uri: &str) -> Result<Lv2PluginInfo> {
    discovery::discover_bundle(bundle_dir)
        .into_iter()
        .find(|i| i.uri == uri)
        .ok_or_else(|| anyhow::anyhow!("LV2 plugin {uri} not found in {}", bundle_dir.display()))
}

/// Automatable parameters = the plugin's control input ports, in port order.
/// Read straight from the TTL, so this never loads the binary.
pub fn read_params(bundle_dir: &Path, uri: &str) -> Vec<PluginParam> {
    info_for(bundle_dir, uri).map(|i| params_of(&i)).unwrap_or_default()
}

fn params_of(info: &Lv2PluginInfo) -> Vec<PluginParam> {
    let mut ports: Vec<&Port> = info
        .ports
        .iter()
        .filter(|p| p.kind == PortKind::ControlInput)
        .collect();
    ports.sort_by_key(|p| p.index);
    ports
        .iter()
        .map(|p| PluginParam {
            id: p.index,
            name: if p.name.is_empty() { p.symbol.clone() } else { p.name.clone() },
            min: p.min as f64,
            max: p.max as f64,
            default: p.default as f64,
            // `toggled` says two positions outright; an enumeration has as many
            // as it named; an integer port has one per whole number in range,
            // and only if that is few enough to step through.
            steps: if p.toggled {
                2
            } else if p.enumeration && !p.points.is_empty() {
                p.points.len() as u32
            } else if p.integer {
                ((p.max - p.min).round() as i64 + 1).clamp(0, u32::MAX as i64) as u32
            } else {
                0
            },
            unit: p.unit.clone(),
            points: p.points.iter().map(|(v, l)| (*v as f64, l.clone())).collect(),
        })
        .collect()
}

/// Carla's rack/patchbay wrappers: plugin hosts pretending to be plugins. Like
/// the VST2 one they corrupt the allocator rather than crash cleanly, so they
/// are skipped by name. Everything else goes through
/// `choz_engine::quarantine`, which probes unknown plugins in a child process.
const DENY_URI_PREFIXES: &[&str] = &["http://kxstudio.sf.net/carla/plugins/carla"];

/// Build a live instance of `uri` from the bundle at `bundle_dir`.
fn build(bundle_dir: &Path, uri: &str, sample_rate: u32, max_block: u32) -> Result<Lv2Instance> {
    if DENY_URI_PREFIXES.iter().any(|p| uri.starts_with(p)) {
        bail!("LV2 plugin {uri} is a plugin host itself; choz will not load it");
    }
    let info = info_for(bundle_dir, uri)?;
    let lib = load_library(&info.binary_path)?;
    // One URID map per instance keeps each source self-contained, so it can be
    // moved onto the audio thread on its own.
    let urids = Arc::new(Mutex::new(UridStore::new()));
    build_instance(info, lib, urids, sample_rate, max_block)
}

impl Lv2Instance {
    /// Set control input `index` (into [`params_of`]) from a 0..1 knob position.
    /// RT-safe: writes the port's own f32 cell, which the plugin reads on `run`.
    fn set_param_norm(&mut self, params: &[PluginParam], index: usize, value: f32) {
        let Some(info) = params.get(index) else { return };
        let Some(cell) = self.control_values.get_mut(info.id as usize) else { return };
        *cell = info.plain(value.clamp(0.0, 1.0) as f64) as f32;
    }

    /// Apply every parameter's TTL default. Plugins whose ports we never touch
    /// otherwise start at 0.0, which is silence (or worse) for most of them.
    fn apply_defaults(&mut self, params: &[PluginParam]) {
        for p in params {
            if let Some(cell) = self.control_values.get_mut(p.id as usize) {
                *cell = p.default as f32;
            }
        }
    }

    /// Run one block of interleaved stereo through the plugin, in place.
    /// `frames <= block_size`. Non-finite output is dropped, never mixed.
    fn process_interleaved(&mut self, block: &mut [f32], wet: f32) {
        let frames = (block.len() / 2).min(self.block_size);
        for (ch, &pi) in self.audio_in.iter().enumerate() {
            let mono = self.audio_in.len() == 1;
            let buf = &mut self.audio_bufs[pi];
            for (f, slot) in buf.iter_mut().enumerate().take(frames) {
                *slot = if mono {
                    (block[f * 2] + block[f * 2 + 1]) * 0.5
                } else {
                    block[f * 2 + ch.min(1)]
                };
            }
        }
        self.write_midi_sequence();
        self.reset_atom_outputs();
        self.run(frames);

        let dry = 1.0 - wet;
        let n_out = self.audio_out.len();
        for f in 0..frames {
            let (l, r) = match n_out {
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
            let (l, r) = if l.is_finite() && r.is_finite() { (l, r) } else { (0.0, 0.0) };
            block[f * 2] = block[f * 2] * dry + l * wet;
            block[f * 2 + 1] = block[f * 2 + 1] * dry + r * wet;
        }
    }
}

/// A live LV2 instrument in a rack slot: notes in, interleaved stereo out.
/// The UI's writes, as choz addresses parameters.
///
/// An LV2 UI reports the **port** it wrote and the value in the port's own
/// units; choz's knobs are positions in the parameter list, normalised 0..1.
/// The translation belongs here, next to the list that knows both.
struct Lv2Touch {
    raw: Arc<parking_lot::Mutex<Option<(u32, f32)>>>,
    params: Vec<PluginParam>,
}

impl choz_ports::ParamTouch for Lv2Touch {
    fn take_touched(&self) -> Option<(u32, f32)> {
        let (port, plain) = self.raw.lock().take()?;
        let index = self.params.iter().position(|p| p.id == port)?;
        Some((index as u32, self.params[index].normalised(plain as f64) as f32))
    }
}

fn touch_of(
    editor: &Option<Arc<editor::Lv2Editor>>,
    params: &[PluginParam],
) -> Option<choz_ports::TouchHandle> {
    let ed = editor.as_ref()?;
    Some(Arc::new(Lv2Touch { raw: ed.touched(), params: params.to_vec() }) as choz_ports::TouchHandle)
}

pub struct Lv2Instrument {
    inst: Lv2Instance,
    params: Vec<PluginParam>,
    /// Built once at load: `editor()` is called from the UI thread and must not
    /// dlopen anything, and building it here is also what makes the GUI button
    /// appear only for plugins that really have a window.
    editor: Option<Arc<editor::Lv2Editor>>,
}

impl Lv2Instrument {
    /// Load an LV2 instrument (a plugin with a MIDI input). `None` on any
    /// failure — a broken plugin must never take the app down.
    pub fn build(bundle_dir: &Path, uri: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        let mut inst = match build(bundle_dir, uri, sample_rate, max_block) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("choz: LV2 {uri}: {e}");
                return None;
            }
        };
        if inst.atom_in.is_none() {
            eprintln!("choz: LV2 {uri} has no MIDI input; not an instrument");
            return None;
        }
        let params = params_of(&inst.info);
        inst.apply_defaults(&params);
        let editor = build_editor(&inst, sample_rate);
        Some(Self { inst, params, editor })
    }
}

/// Load the bundle's X11 editor, if it has one that works.
fn build_editor(inst: &Lv2Instance, sample_rate: u32) -> Option<Arc<editor::Lv2Editor>> {
    let ui = inst.info.x11_ui.as_ref()?;
    editor::Lv2Editor::load(
        ui,
        &inst.info.uri,
        &inst.info.bundle_dir,
        Arc::clone(&inst.controls),
        sample_rate,
    )
}

impl AudioSource for Lv2Instrument {
    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.editor.clone().map(|e| e as choz_ports::EditorHandle)
    }

    fn param_touch(&self) -> Option<choz_ports::TouchHandle> {
        touch_of(&self.editor, &self.params)
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        Some(Arc::new(state::Lv2State { shared: Arc::clone(&self.inst.state) })
            as choz_ports::StateHandle)
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
        self.inst.queue_midi([0xE0, (v & 0x7F) as u8, (v >> 7) as u8]);
    }

    fn program_change(&mut self, bank: u8, preset: u8) {
        self.inst.queue_midi([0xB0, 0x00, bank & 0x7F]); // bank select MSB
        self.inst.queue_midi([0xC0, preset & 0x7F, 0]);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let params = std::mem::take(&mut self.params);
        self.inst.set_param_norm(&params, index, value);
        self.params = params;
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}

/// A live LV2 *audio effect* in a slot's FX chain.
pub struct Lv2Effect {
    inst: Lv2Instance,
    params: Vec<PluginParam>,
    wet: f32,
    /// Built once at load, same as the instrument's.
    editor: Option<Arc<editor::Lv2Editor>>,
}

impl Lv2Effect {
    /// Load an LV2 effect. `None` on any failure (missing bundle, unsupported
    /// required feature, no audio output).
    pub fn build(bundle_dir: &Path, uri: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        let mut inst = match build(bundle_dir, uri, sample_rate, max_block) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("choz: LV2 {uri}: {e}");
                return None;
            }
        };
        if inst.audio_out.is_empty() {
            eprintln!("choz: LV2 {uri} has no audio output; not an effect");
            return None;
        }
        let params = params_of(&inst.info);
        inst.apply_defaults(&params);
        let editor = build_editor(&inst, sample_rate);
        Some(Self { inst, params, wet: 1.0, editor })
    }
}

impl FxProcessor for Lv2Effect {
    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.editor.clone().map(|e| e as choz_ports::EditorHandle)
    }

    fn param_touch(&self) -> Option<choz_ports::TouchHandle> {
        touch_of(&self.editor, &self.params)
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        Some(Arc::new(state::Lv2State { shared: Arc::clone(&self.inst.state) })
            as choz_ports::StateHandle)
    }

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

    fn name(&self) -> &str {
        &self.inst.info.name
    }

    /// The trait's descriptor wants a `'static` name, which a plugin's dynamic
    /// names can't provide — the UI reads them with [`read_params`] instead.
    /// What matters here is the count.
    fn params(&self) -> Vec<choz_ports::FxParam> {
        self.params
            .iter()
            .map(|p| choz_ports::FxParam::new("param", 0.0, p.min as f32, p.max as f32, ""))
            .collect()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let params = std::mem::take(&mut self.params);
        self.inst.set_param_norm(&params, index, value);
        self.params = params;
    }
}
