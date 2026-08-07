//! Real VST3 hosting via the `vst3` COM bindings.
//!
//! Loads a `.vst3` bundle, finds its first Audio Module class, instantiates the
//! component + audio processor + edit controller, and drives it: MIDI note events
//! via a host-provided `IEventList`, stereo `kSample32` output, and parameter
//! read/set through `IEditController`. This gives VST3 instruments the same
//! behaviour as the VST2 host (load → sound → params).
//!
//! Everything here is unsafe COM. Ported from seqterm's `seqterm-plugin-vst3`,
//! minus the native editor (choz is a terminal app). State persistence uses a
//! memory-backed `IBStream`.

use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use libloading::Library;
use vst3::{Class, ComPtr, ComWrapper, Interface};
use vst3::Steinberg::*;
use vst3::Steinberg::Vst::*;


type GetFactoryProc = unsafe extern "system" fn() -> *mut IPluginFactory;

/// Resolve the loadable `.so` inside a Linux `.vst3` bundle, or the file itself.
fn bundle_binary(path: &Path) -> std::path::PathBuf {
    // Linux bundle layout: <name>.vst3/Contents/x86_64-linux/<name>.so
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let arch_dir = path.join("Contents").join("x86_64-linux");
    let so = arch_dir.join(format!("{stem}.so"));
    if so.exists() { return so; }
    // Some bundles use the plain name; fall back to any .so in the arch dir.
    if let Ok(rd) = std::fs::read_dir(&arch_dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "so").unwrap_or(false) { return e.path(); }
        }
    }
    path.to_path_buf()
}

/// Minimal host context passed to `initialize`. Many VST3 plugins call back into
/// this during init/processing; passing null segfaults them, so we provide a real
/// `IHostApplication` (name + a stub `createInstance`).
struct HostContext;

impl Class for HostContext {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if !name.is_null() {
            let dst = &mut *name;
            let src = "choz";
            let mut i = 0;
            for u in src.encode_utf16() { if i + 1 >= dst.len() { break; } dst[i] = u as TChar; i += 1; }
            dst[i] = 0;
        }
        kResultOk
    }
    unsafe fn createInstance(&self, _cid: *mut TUID, _iid: *mut TUID, _obj: *mut *mut c_void) -> tresult {
        kNotImplemented
    }
}

/// Host-provided event list handed to the plugin each process block (read-only:
/// the plugin calls `getEventCount`/`getEvent`).
struct HostEventList {
    events: Vec<Event>,
}

impl Class for HostEventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for HostEventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.len() as int32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match self.events.get(index as usize) {
            Some(ev) => { *e = *ev; kResultOk }
            None => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, _e: *mut Event) -> tresult { kNotImplemented }
}

/// A throwaway value queue: the plugin writes output-automation points here and we
/// discard them. Some plugins (DPF) require `addParameterData` to return a non-null
/// queue, so we hand back this shared instance.
struct HostParamValueQueue;

impl Class for HostParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for HostParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID { 0 }
    unsafe fn getPointCount(&self) -> int32 { 0 }
    unsafe fn getPoint(&self, _index: int32, _off: *mut int32, _value: *mut ParamValue) -> tresult { kResultFalse }
    unsafe fn addPoint(&self, _off: int32, _value: ParamValue, index: *mut int32) -> tresult {
        if !index.is_null() { *index = 0; }
        kResultOk
    }
}

/// Parameter-change list handed to `ProcessData`. Reports zero *input* changes but
/// returns a real (discarding) queue from `addParameterData`, which some plugins
/// (e.g. DPF-based) require to be non-null even with no automation.
struct HostParamChanges {
    queue: ComWrapper<HostParamValueQueue>,
}

impl HostParamChanges {
    fn new() -> Self { Self { queue: ComWrapper::new(HostParamValueQueue) } }
}

impl Class for HostParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for HostParamChanges {
    unsafe fn getParameterCount(&self) -> int32 { 0 }
    unsafe fn getParameterData(&self, _index: int32) -> *mut IParamValueQueue { std::ptr::null_mut() }
    unsafe fn addParameterData(&self, _id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        if !index.is_null() { *index = 0; }
        self.queue.to_com_ptr::<IParamValueQueue>().map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut())
    }
}

/// A memory-backed `IBStream` for component state get/set. The plugin reads from
/// / writes into `data` (with an internal cursor); the shared `Arc<Mutex<…>>` lets
/// the host recover the written bytes after `getState`, or seed them before
/// `setState`. Uses `Mutex` for interior mutability (the trait is `&self`).
struct MemStream {
    inner: Arc<std::sync::Mutex<(Vec<u8>, usize)>>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = (data.len().saturating_sub(*pos)).min(num_bytes.max(0) as usize);
        if n > 0 {
            std::ptr::copy_nonoverlapping(data[*pos..].as_ptr(), buffer as *mut u8, n);
            *pos += n;
        }
        if !num_read.is_null() { *num_read = n as int32; }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_written: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = num_bytes.max(0) as usize;
        let src = std::slice::from_raw_parts(buffer as *const u8, n);
        if *pos + n > data.len() { data.resize(*pos + n, 0); }
        data[*pos..*pos + n].copy_from_slice(src);
        *pos += n;
        if !num_written.is_null() { *num_written = n as int32; }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, cur) = &mut *g;
        use IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd};
        let base = match mode {
            x if x == kIBSeekCur as int32 => *cur as int64,
            x if x == kIBSeekEnd as int32 => data.len() as int64,
            _ => 0, // kIBSeekSet
        };
        let np = (base + pos).max(0) as usize;
        *cur = np;
        if !result.is_null() { *result = np as int64; }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() { *pos = self.inner.lock().unwrap().1 as int64; }
        kResultOk
    }
}

fn note_on_event(ch: u8, note: u8, vel: u8) -> Event {
    let mut e: Event = unsafe { std::mem::zeroed() };
    e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
    e.__field0.noteOn = NoteOnEvent {
        channel: ch as int16,
        pitch: note as int16,
        tuning: 0.0,
        velocity: vel as f32 / 127.0,
        length: 0,
        noteId: -1,
    };
    e
}

fn note_off_event(ch: u8, note: u8) -> Event {
    let mut e: Event = unsafe { std::mem::zeroed() };
    e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
    e.__field0.noteOff = NoteOffEvent {
        channel: ch as int16,
        pitch: note as int16,
        velocity: 0.0,
        noteId: -1,
        tuning: 0.0,
    };
    e
}

/// A live VST3 instrument instance.
pub struct Vst3RealInstance {
    // NOTE: field order = drop order. The COM objects MUST release before the
    // library unloads (dlclose), so `_lib` is declared LAST — otherwise Release()
    // runs against unmapped plugin code and segfaults.
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    block: usize,
    /// De-interleaved output channel buffers + their raw pointers.
    out_bufs: Vec<Vec<f32>>,
    out_ptrs: Vec<*mut f32>,
    /// De-interleaved input channel buffers + raw pointers (effects only; empty for
    /// instruments where `in_channels == 0`).
    in_bufs: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    /// Input-audio-bus channel count (0 = no audio input = instrument).
    in_channels: usize,
    /// MIDI queued since the last `render`, applied at the next process block.
    pending: Vec<Event>,
    /// Actual output-bus channel count reported by the plugin (≥ 1).
    out_channels: usize,
    /// Host context handed to the plugin; kept alive for the instance's lifetime.
    _ctx: ComWrapper<HostContext>,
    /// Empty parameter-change list (non-null pointer for `ProcessData`).
    param_changes: ComWrapper<HostParamChanges>,
    /// Keeps the `.so` mapped; declared last so it unloads after every COM release.
    _lib: Arc<Library>,
}

// SAFETY: COM pointers are used only from the single audio-loop thread that owns
// this instance (mirrors the VST2 host's `unsafe impl Send`).
unsafe impl Send for Vst3RealInstance {}

impl Vst3RealInstance {
    pub fn load(path: &Path, sample_rate: u32, block: u32) -> Result<Self> {
        let bin = bundle_binary(path);
        let lib = unsafe { Library::new(&bin) }
            .with_context(|| format!("load vst3 binary {}", bin.display()))?;
        // Linux VST3 modules MUST be initialised via `ModuleEntry` before the
        // factory is usable — it constructs the module's global state (class info
        // strings, etc.). Skipping it makes `getClassInfo` deref uninitialised
        // memory and segfault. (macOS: `bundleEntry`, Windows: `InitDll`.)
        unsafe {
            type ModuleEntryProc = unsafe extern "system" fn(*mut c_void) -> bool;
            if let Ok(entry) = lib.get::<ModuleEntryProc>(b"ModuleEntry\0") {
                if !entry(std::ptr::null_mut()) {
                    bail!("VST3 ModuleEntry failed");
                }
            }
        }
        let get_factory: libloading::Symbol<GetFactoryProc> = unsafe {
            lib.get(b"GetPluginFactory\0").context("no GetPluginFactory export")?
        };
        let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(get_factory()) }
            .context("GetPluginFactory returned null")?;

        // Find the first "Audio Module Class" and instantiate its IComponent.
        let count = unsafe { factory.countClasses() };
        let mut component: Option<ComPtr<IComponent>> = None;
        for i in 0..count {
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            if unsafe { factory.getClassInfo(i, &mut info) } != kResultOk { continue; }
            let category = c_arr_to_string(&info.category);
            if category != "Audio Module Class" { continue; }
            let mut obj: *mut c_void = std::ptr::null_mut();
            let r = unsafe {
                factory.createInstance(
                    info.cid.as_ptr(),
                    IComponent::IID.as_ptr() as FIDString,
                    &mut obj,
                )
            };
            if r == kResultOk && !obj.is_null() {
                component = unsafe { ComPtr::<IComponent>::from_raw(obj as *mut IComponent) };
                break;
            }
        }
        let component = component.context("no instantiable Audio Module class in bundle")?;

        // IPluginBase::initialize with a real host context (null segfaults many
        // plugins that call back during init).
        let ctx = ComWrapper::new(HostContext);
        let ctx_unknown = ctx.to_com_ptr::<FUnknown>()
            .context("host context FUnknown")?;
        unsafe {
            if component.initialize(ctx_unknown.as_ptr()) != kResultOk {
                bail!("IComponent::initialize failed");
            }
        }

        let processor: ComPtr<IAudioProcessor> =
            component.cast().context("plugin has no IAudioProcessor")?;

        // The edit controller may be the same object (single-component plugins) OR a
        // separate class (the common case, e.g. DPF). For the latter, instantiate it
        // via `getControllerClassId`, initialize it, and connect the two so param
        // edits propagate to the processor.
        let controller: Option<ComPtr<IEditController>> = match component.cast() {
            Some(c) => Some(c),
            None => unsafe {
                let mut cid: TUID = std::mem::zeroed();
                if component.getControllerClassId(&mut cid) == kResultOk {
                    let mut obj: *mut c_void = std::ptr::null_mut();
                    let r = factory.createInstance(
                        cid.as_ptr(),
                        IEditController::IID.as_ptr() as FIDString,
                        &mut obj,
                    );
                    if r == kResultOk && !obj.is_null() {
                        let ctrl = ComPtr::<IEditController>::from_raw(obj as *mut IEditController);
                        if let Some(c) = &ctrl {
                            c.initialize(ctx_unknown.as_ptr());
                            // Connect component ↔ controller so edits sync to audio.
                            if let (Some(cp_comp), Some(cp_ctrl)) =
                                (component.cast::<IConnectionPoint>(), c.cast::<IConnectionPoint>())
                            {
                                cp_comp.connect(cp_ctrl.as_ptr());
                                cp_ctrl.connect(cp_comp.as_ptr());
                            }
                        }
                        ctrl
                    } else { None }
                } else { None }
            },
        };

        // Configure processing.
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            maxSamplesPerBlock: block as int32,
            sampleRate: sample_rate as f64,
        };
        unsafe { processor.setupProcessing(&mut setup); }

        // Query the real output-bus channel count (mono plugins have 1 — hardcoding
        // 2 makes them index out of bounds and assert/crash).
        let out_channels = unsafe {
            let mut bi: BusInfo = std::mem::zeroed();
            if component.getBusInfo(MediaTypes_::kAudio as int32, BusDirections_::kOutput as int32, 0, &mut bi) == kResultOk {
                (bi.channelCount.max(1)) as usize
            } else { 2 }
        };

        // Query the input-audio bus channel count (effects have one; instruments 0).
        let in_channels = unsafe {
            let mut bi: BusInfo = std::mem::zeroed();
            if component.getBusInfo(MediaTypes_::kAudio as int32, BusDirections_::kInput as int32, 0, &mut bi) == kResultOk {
                bi.channelCount.max(0) as usize
            } else { 0 }
        };

        // Activate the main audio-out bus, the event-in bus (notes), and — for
        // effects — the audio-in bus.
        unsafe {
            component.activateBus(MediaTypes_::kAudio as int32, BusDirections_::kOutput as int32, 0, 1);
            component.activateBus(MediaTypes_::kEvent as int32, BusDirections_::kInput as int32, 0, 1);
            if in_channels > 0 {
                component.activateBus(MediaTypes_::kAudio as int32, BusDirections_::kInput as int32, 0, 1);
            }
            component.setActive(1);
            processor.setProcessing(1);
        }

        let out_bufs = vec![vec![0.0f32; block as usize]; out_channels];
        let in_bufs = vec![vec![0.0f32; block as usize]; in_channels];
        Ok(Self {
            _lib: Arc::new(lib),
            component,
            processor,
            controller,
            block: block as usize,
            out_bufs,
            out_ptrs: Vec::with_capacity(out_channels),
            in_bufs,
            in_ptrs: Vec::with_capacity(in_channels),
            in_channels,
            pending: Vec::new(),
            out_channels,
            _ctx: ctx,
            param_changes: ComWrapper::new(HostParamChanges::new()),
        })
    }

    pub fn note_on(&mut self, ch: u8, note: u8, vel: u8) { self.pending.push(note_on_event(ch, note, vel)); }
    pub fn note_off(&mut self, ch: u8, note: u8) { self.pending.push(note_off_event(ch, note)); }

    /// Render one interleaved-stereo block as an instrument (no audio input).
    pub fn render(&mut self, output: &mut [f32]) -> usize {
        self.render_with_input(&[], output)
    }

    /// Render one block, feeding `input` (interleaved stereo) to the plugin's audio
    /// input bus — the effect path. `input` empty ⇒ silent input.
    pub fn render_with_input(&mut self, input: &[f32], output: &mut [f32]) -> usize {
        let frames = (output.len() / 2).min(self.block);
        for b in &mut self.out_bufs { for s in b.iter_mut() { *s = 0.0; } }

        self.out_ptrs.clear();
        for b in &mut self.out_bufs { self.out_ptrs.push(b.as_mut_ptr()); }
        let mut out_bus = AudioBusBuffers {
            numChannels: self.out_channels as int32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 { channelBuffers32: self.out_ptrs.as_mut_ptr() },
        };

        // Fill input channels (deinterleave) when this plugin has an audio-in bus.
        let in_frames = input.len() / 2;
        let mut in_bus;
        let (num_inputs, in_ptr) = if self.in_channels > 0 {
            for (ch_idx, b) in self.in_bufs.iter_mut().enumerate() {
                for (f, v) in b[..frames].iter_mut().enumerate() {
                    *v = if f < in_frames { input[f * 2 + ch_idx.min(1)] } else { 0.0 };
                }
            }
            self.in_ptrs.clear();
            for b in &mut self.in_bufs { self.in_ptrs.push(b.as_mut_ptr()); }
            in_bus = AudioBusBuffers {
                numChannels: self.in_channels as int32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 { channelBuffers32: self.in_ptrs.as_mut_ptr() },
            };
            (1, &mut in_bus as *mut AudioBusBuffers)
        } else {
            (0, std::ptr::null_mut())
        };

        // Host event list for this block (moves the queued notes in).
        let evlist = ComWrapper::new(HostEventList { events: std::mem::take(&mut self.pending) });
        let ev_ptr = evlist.to_com_ptr::<IEventList>().map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut());
        // Non-null (empty) parameter changes — required by some plugins.
        let pc_ptr = self.param_changes.to_com_ptr::<IParameterChanges>()
            .map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut());

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: frames as int32,
            numInputs: num_inputs,
            numOutputs: 1,
            inputs: in_ptr,
            outputs: &mut out_bus,
            inputParameterChanges: pc_ptr,
            outputParameterChanges: pc_ptr,
            inputEvents: ev_ptr,
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };
        unsafe { self.processor.process(&mut data); }

        // Interleave to stereo: duplicate a mono bus to both sides; otherwise take
        // the first two channels.
        let right = if self.out_channels > 1 { 1 } else { 0 };
        for f in 0..frames {
            output[f * 2]     = self.out_bufs[0][f];
            output[f * 2 + 1] = self.out_bufs[right][f];
        }
        frames
    }

    // ── Parameters (via IEditController) ────────────────────────────────────────
    pub fn param_count(&self) -> u32 {
        self.controller.as_ref().map(|c| unsafe { c.getParameterCount() } as u32).unwrap_or(0)
    }
    pub fn get_param(&self, id: u32) -> f32 {
        self.controller.as_ref().map(|c| unsafe { c.getParamNormalized(id) } as f32).unwrap_or(0.0)
    }
    pub fn set_param(&self, id: u32, value: f32) {
        if let Some(c) = &self.controller { unsafe { c.setParamNormalized(id, value as f64); } }
    }
    pub fn param_name(&self, id: u32) -> String {
        let Some(c) = &self.controller else { return format!("P{id}") };
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { c.getParameterInfo(id as int32, &mut info) } == kResultOk {
            w_arr_to_string(&info.title)
        } else {
            format!("P{id}")
        }
    }
    /// Unit label (e.g. "dB", "Hz") from the parameter's `units`.
    pub fn param_label(&self, id: u32) -> String {
        let Some(c) = &self.controller else { return String::new() };
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { c.getParameterInfo(id as int32, &mut info) } == kResultOk {
            w_arr_to_string(&info.units)
        } else {
            String::new()
        }
    }
    /// Formatted display of the current value (via `getParamStringByValue`).
    pub fn param_display(&self, id: u32) -> String {
        let Some(c) = &self.controller else { return String::new() };
        let norm = unsafe { c.getParamNormalized(id) };
        let mut s: String128 = unsafe { std::mem::zeroed() };
        if unsafe { c.getParamStringByValue(id, norm, &mut s) } == kResultOk {
            w_arr_to_string(&s)
        } else {
            String::new()
        }
    }
    // ── State persistence (IComponent get/setState over a memory IBStream) ──────
    /// Serialize the component's state into an opaque blob (empty on failure).
    pub fn get_state(&self) -> Vec<u8> {
        let shared = Arc::new(std::sync::Mutex::new((Vec::new(), 0usize)));
        let stream = ComWrapper::new(MemStream { inner: shared.clone() });
        let Some(ptr) = stream.to_com_ptr::<IBStream>() else { return Vec::new() };
        let ok = unsafe { self.component.getState(ptr.as_ptr()) } == kResultOk;
        if !ok { return Vec::new(); }
        let data = shared.lock().unwrap().0.clone();
        data
    }

    /// Restore component state from a blob produced by [`Self::get_state`]. Also
    /// pushes it to the edit controller so its parameter view syncs.
    pub fn set_state(&self, data: &[u8]) {
        if data.is_empty() { return; }
        // Component state.
        let shared = Arc::new(std::sync::Mutex::new((data.to_vec(), 0usize)));
        let stream = ComWrapper::new(MemStream { inner: shared.clone() });
        if let Some(ptr) = stream.to_com_ptr::<IBStream>() {
            unsafe { self.component.setState(ptr.as_ptr()); }
        }
        // Mirror into the controller (rewind a fresh stream first).
        if let Some(c) = &self.controller {
            let s2 = ComWrapper::new(MemStream { inner: Arc::new(std::sync::Mutex::new((data.to_vec(), 0usize))) });
            if let Some(ptr) = s2.to_com_ptr::<IBStream>() {
                unsafe { c.setComponentState(ptr.as_ptr()); }
            }
        }
    }
}

impl Drop for Vst3RealInstance {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some(c) = &self.controller { c.terminate(); }
            self.component.terminate();
        }
    }
}

/// Convert a fixed `char8` C array to a Rust String (NUL-terminated).
fn c_arr_to_string(buf: &[char8]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Convert a fixed UTF-16 `TChar` array (VST3 String128) to a Rust String.
fn w_arr_to_string(buf: &[TChar]) -> String {
    let units: Vec<u16> = buf.iter().copied().take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&units).trim().to_string()
}


// ─── Factory metadata (scan-time, no instantiation) ─────────────────────────

/// What the plugin factory says about a bundle.
pub struct FactoryInfo {
    pub name: String,
    pub vendor: String,
    /// True when a class declares an `Instrument` sub-category.
    pub is_instrument: bool,
}

/// Read a bundle's factory: plugin name, vendor, and whether it is an
/// instrument. `None` when the bundle can't be opened — the caller falls back
/// to the file name.
pub fn factory_info(path: &Path) -> Option<FactoryInfo> {
    let bin = bundle_binary(path);
    let lib = unsafe { Library::new(&bin) }.ok()?;
    unsafe {
        type ModuleEntryProc = unsafe extern "system" fn(*mut c_void) -> bool;
        if let Ok(entry) = lib.get::<ModuleEntryProc>(b"ModuleEntry\0") {
            if !entry(std::ptr::null_mut()) {
                return None;
            }
        }
    }
    let get_factory: libloading::Symbol<GetFactoryProc> =
        unsafe { lib.get(b"GetPluginFactory\0") }.ok()?;
    let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(get_factory()) }?;

    let mut vendor = String::new();
    let mut fi: PFactoryInfo = unsafe { std::mem::zeroed() };
    if unsafe { factory.getFactoryInfo(&mut fi) } == kResultOk {
        vendor = c_arr_to_string(&fi.vendor);
    }

    // The sub-category string ("Instrument|Synth", "Fx|Delay"…) only exists on
    // IPluginFactory2's class info, so ask for it when the plugin has one.
    let factory2: Option<ComPtr<IPluginFactory2>> = factory.cast();
    let count = unsafe { factory.countClasses() };
    let mut name = String::new();
    let mut is_instrument = false;
    for i in 0..count {
        let mut info: PClassInfo = unsafe { std::mem::zeroed() };
        if unsafe { factory.getClassInfo(i, &mut info) } != kResultOk {
            continue;
        }
        if c_arr_to_string(&info.category) != "Audio Module Class" {
            continue;
        }
        if name.is_empty() {
            name = c_arr_to_string(&info.name);
        }
        if let Some(f2) = &factory2 {
            let mut info2: PClassInfo2 = unsafe { std::mem::zeroed() };
            if unsafe { f2.getClassInfo2(i, &mut info2) } == kResultOk
                && c_arr_to_string(&info2.subCategories).contains("Instrument")
            {
                is_instrument = true;
            }
        }
    }
    Some(FactoryInfo { name, vendor, is_instrument })
}
