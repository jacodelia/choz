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
use choz_ports::EditorHandle;
use libloading::Library;
use vst3::{Class, ComPtr, ComWrapper, Interface};

use crate::editor::{SharedView, Vst3Editor};
use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

type GetFactoryProc = unsafe extern "system" fn() -> *mut IPluginFactory;

/// Resolve the loadable `.so` inside a Linux `.vst3` bundle, or the file itself.
fn bundle_binary(path: &Path) -> std::path::PathBuf {
    // Linux bundle layout: <name>.vst3/Contents/x86_64-linux/<name>.so
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let arch_dir = path.join("Contents").join("x86_64-linux");
    let so = arch_dir.join(format!("{stem}.so"));
    if so.exists() {
        return so;
    }
    // Some bundles use the plain name; fall back to any .so in the arch dir.
    if let Ok(rd) = std::fs::read_dir(&arch_dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "so").unwrap_or(false) {
                return e.path();
            }
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
            for u in src.encode_utf16() {
                if i + 1 >= dst.len() {
                    break;
                }
                dst[i] = u as TChar;
                i += 1;
            }
            dst[i] = 0;
        }
        kResultOk
    }
    /// The only thing a plugin asks the host to build is an `IMessage` — that
    /// is how the two halves of a VST3 plugin (edit controller and processor)
    /// talk to each other. Returning `kNotImplemented` is what made DPF's UI
    /// assert `message != nullptr` the moment its editor opened, so a knob in
    /// the plugin's own window never reached the DSP.
    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the plugin passes a readable TUID and a writable out-pointer.
        let wanted = unsafe { *iid };
        if wanted != IMessage_iid && wanted != FUnknown_iid {
            return kNotImplemented;
        }
        let msg = ComWrapper::new(HostMessage::new());
        let Some(ptr) = msg.to_com_ptr::<IMessage>() else {
            return kInternalError;
        };
        // The caller owns the reference `to_com_ptr` just took.
        unsafe { *obj = ptr.into_raw() as *mut c_void };
        kResultOk
    }
}

/// A host-created `IMessage`: an id plus an attribute list, passed between the
/// plugin's controller and processor through `IConnectionPoint`.
struct HostMessage {
    id: std::sync::Mutex<std::ffi::CString>,
    attrs: ComWrapper<HostAttributeList>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            id: std::sync::Mutex::new(std::ffi::CString::default()),
            attrs: ComWrapper::new(HostAttributeList::default()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        // The pointer stays valid as long as the message does: the CString is
        // only replaced under the same lock, and a plugin reads the id inside
        // the callback that received the message.
        self.id.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let new = if id.is_null() {
            std::ffi::CString::default()
        } else {
            // SAFETY: VST3 message ids are NUL-terminated C strings.
            unsafe { std::ffi::CStr::from_ptr(id) }.to_owned()
        };
        *self.id.lock().unwrap_or_else(|e| e.into_inner()) = new;
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // Borrowed, not owned: the message holds the only reference.
        self.attrs
            .to_com_ptr::<IAttributeList>()
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
}

/// The message's payload. Values are stored by key, exactly as set — no
/// conversion between the typed getters, which is what the SDK's own
/// implementation does.
#[derive(Default)]
struct HostAttributeList {
    values: std::sync::Mutex<std::collections::HashMap<Vec<u8>, AttrValue>>,
}

enum AttrValue {
    Int(int64),
    Float(f64),
    /// UTF-16, NUL-terminated, as VST3 strings are.
    Text(Vec<u16>),
    Binary(Vec<u8>),
}

impl HostAttributeList {
    fn key(id: AttrID) -> Option<Vec<u8>> {
        (!id.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes().to_vec())
    }

    fn set(&self, id: AttrID, value: AttrValue) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, value);
        kResultOk
    }
}

impl Class for HostAttributeList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for HostAttributeList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        self.set(id, AttrValue::Int(value))
    }

    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        let g = self.values.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(&key) {
            Some(AttrValue::Int(v)) if !value.is_null() => {
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        self.set(id, AttrValue::Float(value))
    }

    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        let g = self.values.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(&key) {
            Some(AttrValue::Float(v)) if !value.is_null() => {
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a VST3 string attribute is NUL-terminated UTF-16.
        let mut buf = Vec::new();
        let mut p = string;
        unsafe {
            while *p != 0 {
                buf.push(*p);
                p = p.add(1);
            }
        }
        buf.push(0);
        self.set(id, AttrValue::Text(buf))
    }

    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        let g = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let Some(AttrValue::Text(v)) = g.get(&key) else {
            return kResultFalse;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        // `sizeInBytes` counts bytes, and each character is two of them.
        let room = (size_in_bytes as usize) / 2;
        if room == 0 {
            return kResultFalse;
        }
        let n = v.len().min(room);
        // SAFETY: the caller promised `room` characters of writable space.
        unsafe {
            for (i, c) in v[..n].iter().enumerate() {
                *string.add(i) = *c as TChar;
            }
            *string.add(n - 1) = 0;
        }
        kResultOk
    }

    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        if data.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller promised `size_in_bytes` readable bytes.
        let bytes =
            unsafe { std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize) };
        self.set(id, AttrValue::Binary(bytes.to_vec()))
    }

    unsafe fn getBinary(
        &self,
        id: AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        let g = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let Some(AttrValue::Binary(v)) = g.get(&key) else {
            return kResultFalse;
        };
        if data.is_null() || size_in_bytes.is_null() {
            return kInvalidArgument;
        }
        // The pointer stays owned by this list, which is how the SDK's own
        // attribute list behaves: valid until the attribute is overwritten.
        unsafe {
            *data = v.as_ptr() as *const c_void;
            *size_in_bytes = v.len() as uint32;
        }
        kResultOk
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
            Some(ev) => {
                *e = *ev;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        kNotImplemented
    }
}

/// One parameter change for one block: the id and the value, as a single point
/// at offset 0.
///
/// Not a throwaway any more. A VST3 plugin's GUI lives in the **edit
/// controller**, which is a different object from the processor: moving a knob
/// calls `IComponentHandler::performEdit` on the host and nothing else. If the
/// host does not carry that value into `ProcessData.inputParameterChanges`, the
/// knob moves on screen and the sound never changes — which is exactly what
/// Surge XT did here.
struct HostParamValueQueue {
    id: std::sync::atomic::AtomicU32,
    value: std::sync::Mutex<f64>,
}

impl HostParamValueQueue {
    fn new() -> Self {
        Self {
            id: std::sync::atomic::AtomicU32::new(0),
            value: std::sync::Mutex::new(0.0),
        }
    }

    fn set(&self, id: u32, value: f64) {
        self.id.store(id, std::sync::atomic::Ordering::Relaxed);
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }
}

impl Class for HostParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for HostParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.load(std::sync::atomic::Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(&self, index: int32, off: *mut int32, value: *mut ParamValue) -> tresult {
        if index != 0 {
            return kResultFalse;
        }
        // The whole change applies at the start of the block: choz has no
        // sample-accurate automation to place it anywhere else.
        if !off.is_null() {
            unsafe { *off = 0 };
        }
        if !value.is_null() {
            unsafe { *value = *self.value.lock().unwrap_or_else(|e| e.into_inner()) };
        }
        kResultOk
    }
    /// Output automation from the plugin. Accepted and dropped: choz reads the
    /// same edits through `IComponentHandler`, which is where a GUI reports
    /// them.
    unsafe fn addPoint(&self, _off: int32, _value: ParamValue, index: *mut int32) -> tresult {
        if !index.is_null() {
            unsafe { *index = 0 };
        }
        kResultOk
    }
}

/// The parameter-change list handed to `ProcessData` each block.
///
/// Queues are pooled and reused: a block reports the first `active` of them, so
/// nothing is allocated on the audio thread once the pool has grown to the
/// number of parameters that move at once.
struct HostParamChanges {
    queues: std::sync::Mutex<Vec<ComWrapper<HostParamValueQueue>>>,
    active: std::sync::atomic::AtomicUsize,
}

impl HostParamChanges {
    fn new() -> Self {
        Self {
            queues: std::sync::Mutex::new(Vec::new()),
            active: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Load this block's changes. Grows the pool the first time a new number of
    /// simultaneous changes shows up, and never shrinks it.
    fn load(&self, changes: &[(u32, f64)]) {
        let mut queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        while queues.len() < changes.len() {
            queues.push(ComWrapper::new(HostParamValueQueue::new()));
        }
        for (q, (id, v)) in queues.iter().zip(changes) {
            q.set(*id, *v);
        }
        self.active
            .store(changes.len(), std::sync::atomic::Ordering::Relaxed);
    }
}

impl Class for HostParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for HostParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.active.load(std::sync::atomic::Ordering::Relaxed) as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index < 0 || index as usize >= self.active.load(std::sync::atomic::Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        let queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        queues
            .get(index as usize)
            .and_then(|q| q.to_com_ptr::<IParamValueQueue>())
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
    /// The plugin asking for somewhere to write *output* automation. It gets a
    /// real queue (some DPF plugins insist on a non-null one) whose points are
    /// discarded.
    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if !index.is_null() {
            unsafe { *index = 0 };
        }
        let mut queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        if queues.is_empty() {
            queues.push(ComWrapper::new(HostParamValueQueue::new()));
        }
        queues[0]
            .to_com_ptr::<IParamValueQueue>()
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
}

/// The plugin's component and controller, reachable from the UI thread for
/// state save/restore.
///
/// Same contract as the editor's cell: `None` once the instance is gone, so a
/// project saved after a tab was replaced reads nothing rather than freed
/// memory.
pub type SharedState = Arc<std::sync::Mutex<Option<StateCell>>>;

pub struct StateCell {
    component: ComPtr<IComponent>,
    controller: Option<ComPtr<IEditController>>,
    _lib: Arc<Library>,
}

impl StateCell {
    /// The controller, for whoever else needs it — the preset browser reads its
    /// program lists through the same cell the state does.
    pub fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.controller.as_ref()
    }
}

// SAFETY: only touched under the mutex, from the UI thread, while the instance
// is alive — which is exactly what `Some` marks.
unsafe impl Send for StateCell {}

pub struct Vst3State {
    pub shared: SharedState,
}

impl choz_ports::PluginState for Vst3State {
    fn save(&self) -> Option<Vec<u8>> {
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let cell = guard.as_ref()?;
        let shared = Arc::new(std::sync::Mutex::new((Vec::new(), 0usize)));
        let stream = ComWrapper::new(MemStream {
            inner: shared.clone(),
        });
        let ptr = stream.to_com_ptr::<IBStream>()?;
        // SAFETY: live component under the mutex; the stream is ours.
        if unsafe { cell.component.getState(ptr.as_ptr()) } != kResultOk {
            return None;
        }
        let data = shared.lock().unwrap_or_else(|e| e.into_inner()).0.clone();
        (!data.is_empty()).then_some(data)
    }

    fn restore(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        let feed = |bytes: &[u8]| {
            let shared = Arc::new(std::sync::Mutex::new((bytes.to_vec(), 0usize)));
            ComWrapper::new(MemStream { inner: shared }).to_com_ptr::<IBStream>()
        };
        // SAFETY: live objects under the mutex.
        unsafe {
            if let Some(p) = feed(data) {
                cell.component.setState(p.as_ptr());
            }
            // The controller gets its own rewound stream, or its window shows
            // the old patch while the processor plays the new one.
            if let (Some(c), Some(p)) = (&cell.controller, feed(data)) {
                c.setComponentState(p.as_ptr());
            }
        }
    }
}

/// What the plugin's own GUI reports back to the host, shared with whoever needs
/// it: the audio thread (to apply the change) and the UI thread (to know which
/// parameter the user just grabbed, for MIDI learn).
#[derive(Clone, Default)]
pub struct EditFeed {
    inner: Arc<std::sync::Mutex<EditFeedInner>>,
}

#[derive(Default)]
struct EditFeedInner {
    /// Changes not yet handed to the processor.
    pending: Vec<(u32, f64)>,
    /// The last parameter the user moved and its normalised value, in the
    /// plugin's window or in choz's own editor. Read — and cleared — by MIDI
    /// learn and by the UI keeping its own knobs in step.
    last_touched: Option<(u32, f32)>,
}

/// Cap on changes queued between two blocks. A knob sweep generates a lot of
/// them; past this the oldest are dropped, because the newest value is the one
/// that matters.
const MAX_PENDING_EDITS: usize = 512;

impl EditFeed {
    fn lock(&self) -> std::sync::MutexGuard<'_, EditFeedInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a parameter change: it will reach the processor next block, and
    /// it marks the parameter as the one being touched.
    pub fn push(&self, id: u32, value: f64) {
        let mut g = self.lock();
        g.last_touched = Some((id, value as f32));
        if g.pending.len() >= MAX_PENDING_EDITS {
            g.pending.remove(0);
        }
        g.pending.push((id, value));
    }

    /// Take everything queued, for one process block.
    fn drain(&self, out: &mut Vec<(u32, f64)>) {
        out.clear();
        let mut g = self.lock();
        out.append(&mut g.pending);
    }

    /// The parameter the user last moved, if any. Cleared by reading, so a
    /// second call returns `None` until something else is touched.
    pub fn take_last_touched(&self) -> Option<(u32, f32)> {
        self.lock().last_touched.take()
    }
}

/// The edit feed as choz sees it: **by parameter index**, not by the plugin's
/// own id. Everything above the host — knob rows, MIDI-learn targets, saved
/// projects — addresses parameters by their position in the list, so the
/// translation belongs here, next to the table that knows it.
pub struct TouchByIndex {
    feed: EditFeed,
    ids: Vec<u32>,
}

impl choz_ports::ParamTouch for TouchByIndex {
    fn take_touched(&self) -> Option<(u32, f32)> {
        let (id, value) = self.feed.take_last_touched()?;
        let index = self.ids.iter().position(|&p| p == id)?;
        Some((index as u32, value))
    }
}

/// The host object a VST3 edit controller reports GUI edits to.
struct HostComponentHandler {
    feed: EditFeed,
}

impl Class for HostComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for HostComponentHandler {
    /// Start of a gesture (mouse down on a knob). Nothing to do: choz has no
    /// automation to arm, and the value arrives in `performEdit`.
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value_normalized: ParamValue) -> tresult {
        self.feed.push(id, value_normalized);
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    /// The plugin wants the host to re-read something (parameter list, latency,
    /// …). choz reads parameters on demand, so acknowledging is enough.
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
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
        if !num_read.is_null() {
            *num_read = n as int32;
        }
        kResultOk
    }
    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_written: *mut int32,
    ) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = num_bytes.max(0) as usize;
        let src = std::slice::from_raw_parts(buffer as *const u8, n);
        if *pos + n > data.len() {
            data.resize(*pos + n, 0);
        }
        data[*pos..*pos + n].copy_from_slice(src);
        *pos += n;
        if !num_written.is_null() {
            *num_written = n as int32;
        }
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
        if !result.is_null() {
            *result = np as int64;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            *pos = self.inner.lock().unwrap().1 as int64;
        }
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
/// choz's clock as VST3 wants it, filled from [`choz_ports::transport`].
///
/// Only what choz actually knows is flagged valid: tempo, the musical position
/// the time signature and the bar the playhead is in. There is no arrangement
/// here, so no cycle and no SMPTE — a plugin reads a field only when its flag
/// says it is there.
///
/// The bar position is the *phase*, not a place in a song: choz counts bars
/// from wherever the transport was last reset. That is what a plugin syncing a
/// pattern to bar starts actually needs, and it is true, which "bar 1 forever"
/// was not.
fn host_process_context() -> ProcessContext {
    let t = choz_ports::transport();
    // SAFETY: `ProcessContext` is a plain C struct of numbers; zero is its
    // "nothing known" state, and every field choz knows is written below.
    let mut ctx: ProcessContext = unsafe { std::mem::zeroed() };
    ctx.sampleRate = t.sample_rate() as f64;
    ctx.projectTimeSamples = t.samples() as TSamples;
    ctx.continousTimeSamples = t.samples() as TSamples;
    ctx.projectTimeMusic = t.ppq() as TQuarterNotes;
    ctx.tempo = t.bpm() as f64;
    let (num, den) = t.time_signature();
    ctx.timeSigNumerator = num as int32;
    ctx.timeSigDenominator = den as int32;
    ctx.barPositionMusic = t.bar_position().1 as TQuarterNotes;
    ctx.state = ProcessContext_::StatesAndFlags_::kBarPositionValid
        | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
        | ProcessContext_::StatesAndFlags_::kTempoValid
        | ProcessContext_::StatesAndFlags_::kTimeSigValid
        | ProcessContext_::StatesAndFlags_::kContTimeValid
        | if t.playing() {
            ProcessContext_::StatesAndFlags_::kPlaying
        } else {
            0
        };
    ctx
}

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
    /// The controller's parameters in order, as `(id, index)` — VST3 parameter
    /// **ids are not indices**. `getParameterInfo` takes an index and hands
    /// back an arbitrary `ParamID`, and every value call (`getParamNormalized`,
    /// `setParamNormalized`, automation queues) takes that id. Surge XT and
    /// most JUCE plugins number theirs in a way that made "index as id" address
    /// the wrong parameter, or none at all.
    param_ids: Vec<u32>,
    /// The parameter changes handed to the plugin each block.
    param_changes: ComWrapper<HostParamChanges>,
    /// Edits reported by the plugin's own GUI (and by choz's knobs), on their
    /// way to the processor.
    edits: EditFeed,
    /// Scratch for one block's worth of changes; pre-allocated so `render` does
    /// not allocate.
    edit_scratch: Vec<(u32, f64)>,
    /// The handler the controller reports edits to; kept alive for as long as
    /// the controller can call it.
    _handler: ComWrapper<HostComponentHandler>,
    /// The plugin's `IPlugView`, reachable from the editor thread. Emptied by
    /// `Drop` before anything is terminated.
    shared_view: SharedView,
    /// The component and controller, for saving and restoring the plugin's own
    /// state. Emptied by `Drop` alongside the view.
    shared_state: SharedState,
    /// Keeps the `.so` mapped; declared last so it unloads after every COM release.
    _lib: Arc<Library>,
}

// SAFETY: COM pointers are used only from the single audio-loop thread that owns
// this instance (mirrors the VST2 host's `unsafe impl Send`).
unsafe impl Send for Vst3RealInstance {}

/// Most steps worth asking a plugin to name, one `getParamStringByValue` call
/// each. Beyond this a stepped parameter is a range to slide through, not a
/// list of choices to pick from.
const MAX_NAMED_STEPS: u32 = 32;

impl Vst3RealInstance {
    pub fn load(path: &Path, sample_rate: u32, block: u32) -> Result<Self> {
        let bin = bundle_binary(path);
        let lib = Arc::new(
            unsafe { Library::new(&bin) }
                .with_context(|| format!("load vst3 binary {}", bin.display()))?,
        );
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
            lib.get(b"GetPluginFactory\0")
                .context("no GetPluginFactory export")?
        };
        let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(get_factory()) }
            .context("GetPluginFactory returned null")?;

        // Find the first "Audio Module Class" and instantiate its IComponent.
        let count = unsafe { factory.countClasses() };
        let mut component: Option<ComPtr<IComponent>> = None;
        for i in 0..count {
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            if unsafe { factory.getClassInfo(i, &mut info) } != kResultOk {
                continue;
            }
            let category = c_arr_to_string(&info.category);
            if category != "Audio Module Class" {
                continue;
            }
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
        let ctx_unknown = ctx
            .to_com_ptr::<FUnknown>()
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
                            if let (Some(cp_comp), Some(cp_ctrl)) = (
                                component.cast::<IConnectionPoint>(),
                                c.cast::<IConnectionPoint>(),
                            ) {
                                cp_comp.connect(cp_ctrl.as_ptr());
                                cp_ctrl.connect(cp_comp.as_ptr());
                            }
                        }
                        ctrl
                    } else {
                        None
                    }
                } else {
                    None
                }
            },
        };

        // The parameter table, read once: index → the plugin's own id.
        let param_ids: Vec<u32> = controller
            .as_ref()
            .map(|c| {
                let n = unsafe { c.getParameterCount() }.max(0);
                (0..n)
                    .filter_map(|i| {
                        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                        // SAFETY: live controller, index in range.
                        (unsafe { c.getParameterInfo(i, &mut info) } == kResultOk)
                            .then_some(info.id)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // The controller reports GUI edits here. Without a handler, a plugin
        // whose window is open moves its own knobs and the processor never
        // hears about it.
        let edits = EditFeed::default();
        let handler = ComWrapper::new(HostComponentHandler {
            feed: edits.clone(),
        });
        if let (Some(c), Some(h)) = (&controller, handler.to_com_ptr::<IComponentHandler>()) {
            // SAFETY: live controller, and the handler outlives it (both are
            // owned by this instance, dropped after `terminate`).
            unsafe { c.setComponentHandler(h.as_ptr()) };
        }

        // Configure processing.
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            maxSamplesPerBlock: block as int32,
            sampleRate: sample_rate as f64,
        };
        unsafe {
            processor.setupProcessing(&mut setup);
        }

        // Query the real output-bus channel count (mono plugins have 1 — hardcoding
        // 2 makes them index out of bounds and assert/crash).
        let out_channels = unsafe {
            let mut bi: BusInfo = std::mem::zeroed();
            if component.getBusInfo(
                MediaTypes_::kAudio as int32,
                BusDirections_::kOutput as int32,
                0,
                &mut bi,
            ) == kResultOk
            {
                (bi.channelCount.max(1)) as usize
            } else {
                2
            }
        };

        // Query the input-audio bus channel count (effects have one; instruments 0).
        let in_channels = unsafe {
            let mut bi: BusInfo = std::mem::zeroed();
            if component.getBusInfo(
                MediaTypes_::kAudio as int32,
                BusDirections_::kInput as int32,
                0,
                &mut bi,
            ) == kResultOk
            {
                bi.channelCount.max(0) as usize
            } else {
                0
            }
        };

        // Activate the main audio-out bus, the event-in bus (notes), and — for
        // effects — the audio-in bus.
        unsafe {
            component.activateBus(
                MediaTypes_::kAudio as int32,
                BusDirections_::kOutput as int32,
                0,
                1,
            );
            component.activateBus(
                MediaTypes_::kEvent as int32,
                BusDirections_::kInput as int32,
                0,
                1,
            );
            if in_channels > 0 {
                component.activateBus(
                    MediaTypes_::kAudio as int32,
                    BusDirections_::kInput as int32,
                    0,
                    1,
                );
            }
            component.setActive(1);
            processor.setProcessing(1);
        }

        // The plugin's editor, created once here — the controller is only
        // reachable on this thread, before the instance moves to the audio one.
        // A plugin without an X11 view gets no cell, and so no `GUI` button.
        let shared_view = Arc::new(std::sync::Mutex::new(controller.as_ref().and_then(|c| {
            // SAFETY: live controller; `createView` returns a +1 reference that
            // `from_raw` takes over.
            let raw = unsafe { c.createView(ViewType::kEditor) };
            let view = unsafe { ComPtr::<IPlugView>::from_raw(raw) }?;
            Vst3Editor::cell(view, Arc::clone(&lib))
        })));

        let shared_state = Arc::new(std::sync::Mutex::new(Some(StateCell {
            component: component.clone(),
            controller: controller.clone(),
            _lib: Arc::clone(&lib),
        })));

        let out_bufs = vec![vec![0.0f32; block as usize]; out_channels];
        let in_bufs = vec![vec![0.0f32; block as usize]; in_channels];
        Ok(Self {
            _lib: lib,
            shared_view,
            shared_state,
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
            param_ids,
            param_changes: ComWrapper::new(HostParamChanges::new()),
            edits,
            edit_scratch: Vec::with_capacity(64),
            _handler: handler,
        })
    }

    /// The plugin's own state blob: its patch, not just its parameter values.
    /// The plugin's program lists, when it has any and a way to select them.
    pub fn presets(&self) -> Option<choz_ports::PresetsHandle> {
        crate::presets::Vst3Presets::new(Arc::clone(&self.shared_state), self.edits.clone())
            .map(|p| Arc::new(p) as choz_ports::PresetsHandle)
    }

    pub fn state(&self) -> Option<choz_ports::StateHandle> {
        Some(Arc::new(Vst3State {
            shared: Arc::clone(&self.shared_state),
        }) as choz_ports::StateHandle)
    }

    /// Handle to the plugin's own window, or `None` if it has no X11 editor.
    pub fn editor(&self) -> Option<EditorHandle> {
        let has_view = self
            .shared_view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        has_view.then(|| Vst3Editor::new(Arc::clone(&self.shared_view)) as EditorHandle)
    }

    pub fn note_on(&mut self, ch: u8, note: u8, vel: u8) {
        self.pending.push(note_on_event(ch, note, vel));
    }
    pub fn note_off(&mut self, ch: u8, note: u8) {
        self.pending.push(note_off_event(ch, note));
    }

    /// Render one interleaved-stereo block as an instrument (no audio input).
    pub fn render(&mut self, output: &mut [f32]) -> usize {
        self.render_with_input(&[], output)
    }

    /// Render one block, feeding `input` (interleaved stereo) to the plugin's audio
    /// input bus — the effect path. `input` empty ⇒ silent input.
    pub fn render_with_input(&mut self, input: &[f32], output: &mut [f32]) -> usize {
        let frames = (output.len() / 2).min(self.block);
        for b in &mut self.out_bufs {
            for s in b.iter_mut() {
                *s = 0.0;
            }
        }

        self.out_ptrs.clear();
        for b in &mut self.out_bufs {
            self.out_ptrs.push(b.as_mut_ptr());
        }
        let mut out_bus = AudioBusBuffers {
            numChannels: self.out_channels as int32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.out_ptrs.as_mut_ptr(),
            },
        };

        // Fill input channels (deinterleave) when this plugin has an audio-in bus.
        let in_frames = input.len() / 2;
        let mut in_bus;
        let (num_inputs, in_ptr) = if self.in_channels > 0 {
            for (ch_idx, b) in self.in_bufs.iter_mut().enumerate() {
                for (f, v) in b[..frames].iter_mut().enumerate() {
                    *v = if f < in_frames {
                        input[f * 2 + ch_idx.min(1)]
                    } else {
                        0.0
                    };
                }
            }
            self.in_ptrs.clear();
            for b in &mut self.in_bufs {
                self.in_ptrs.push(b.as_mut_ptr());
            }
            in_bus = AudioBusBuffers {
                numChannels: self.in_channels as int32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: self.in_ptrs.as_mut_ptr(),
                },
            };
            (1, &mut in_bus as *mut AudioBusBuffers)
        } else {
            (0, std::ptr::null_mut())
        };

        // Host event list for this block (moves the queued notes in).
        let evlist = ComWrapper::new(HostEventList {
            events: std::mem::take(&mut self.pending),
        });
        let ev_ptr = evlist
            .to_com_ptr::<IEventList>()
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut());
        // Everything the GUI (or a choz knob) changed since the last block, as
        // real input automation. This is what makes a plugin's own window
        // actually change the sound.
        self.edits.drain(&mut self.edit_scratch);
        self.param_changes.load(&self.edit_scratch);
        let pc_ptr = self
            .param_changes
            .to_com_ptr::<IParameterChanges>()
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut());

        // choz's clock. VST3 takes it by pointer per block, and a plugin that
        // syncs anything reads it there; a null one (which is what this was)
        // means "the host has no idea what time it is".
        let mut context = host_process_context();
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
            processContext: &mut context,
        };
        unsafe {
            self.processor.process(&mut data);
        }

        // Interleave to stereo: duplicate a mono bus to both sides; otherwise take
        // the first two channels.
        let right = if self.out_channels > 1 { 1 } else { 0 };
        for f in 0..frames {
            output[f * 2] = self.out_bufs[0][f];
            output[f * 2 + 1] = self.out_bufs[right][f];
        }
        frames
    }

    // ── Parameters (via IEditController) ────────────────────────────────────────
    pub fn param_count(&self) -> u32 {
        self.param_ids.len() as u32
    }

    /// The plugin's id for the parameter at `index`.
    pub fn param_id(&self, index: u32) -> Option<u32> {
        self.param_ids.get(index as usize).copied()
    }

    /// Index of a parameter the plugin named by id — the direction a GUI edit
    /// arrives in.
    pub fn param_index(&self, id: u32) -> Option<u32> {
        self.param_ids
            .iter()
            .position(|&p| p == id)
            .map(|i| i as u32)
    }

    pub fn get_param(&self, index: u32) -> f32 {
        let (Some(c), Some(id)) = (self.controller.as_ref(), self.param_id(index)) else {
            return 0.0;
        };
        unsafe { c.getParamNormalized(id) as f32 }
    }
    /// Move a parameter from choz's side (a knob in the RACK, a MIDI-learn
    /// binding, a project being loaded).
    ///
    /// Both halves have to hear about it: the controller so its window shows
    /// the new position, and the processor so the sound follows. The second one
    /// only happens through the block's input parameter changes.
    pub fn set_param(&self, index: u32, value: f32) {
        let Some(id) = self.param_id(index) else {
            return;
        };
        let v = value.clamp(0.0, 1.0) as f64;
        if let Some(c) = &self.controller {
            unsafe {
                c.setParamNormalized(id, v);
            }
        }
        self.edits.push(id, v);
    }

    /// The feed of edits coming from the plugin's own window, translated to
    /// parameter indices. Handed to the UI so MIDI learn can bind whatever knob
    /// the user just grabbed in the plugin's GUI, and so choz's own copy of the
    /// values follows what happens in there.
    pub fn edit_feed(&self) -> TouchByIndex {
        TouchByIndex {
            feed: self.edits.clone(),
            ids: self.param_ids.clone(),
        }
    }
    pub fn param_name(&self, index: u32) -> String {
        let Some(c) = &self.controller else {
            return format!("P{index}");
        };
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { c.getParameterInfo(index as int32, &mut info) } == kResultOk {
            w_arr_to_string(&info.title)
        } else {
            format!("P{index}")
        }
    }
    /// How many positions the parameter has, and their names when it has few
    /// enough to name.
    ///
    /// VST3 counts *intervals*, not positions: `stepCount == 1` is a switch
    /// with two of them, `n` has `n + 1`. `0` is continuous. The names come from
    /// `getParamStringByValue`, which is the only way to learn that step 2 of a
    /// filter-type parameter is called "Bandpass" — the value itself is a
    /// normalised float like everything else.
    pub fn param_steps(&self, index: u32) -> (u32, Vec<(f64, String)>) {
        let Some(c) = &self.controller else {
            return (0, Vec::new());
        };
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { c.getParameterInfo(index as int32, &mut info) } != kResultOk {
            return (0, Vec::new());
        }
        let steps = info.stepCount;
        if steps <= 0 {
            return (0, Vec::new());
        }
        let count = steps as u32 + 1;
        // A switch draws as a switch and needs no names; a long list of steps
        // is a fader, and asking a plugin for 128 strings on every load is not
        // worth what it buys.
        if count == 2 || count > MAX_NAMED_STEPS {
            return (count, Vec::new());
        }
        let points = (0..count)
            .filter_map(|k| {
                let norm = k as f64 / steps as f64;
                let mut s: String128 = unsafe { std::mem::zeroed() };
                (unsafe { c.getParamStringByValue(info.id, norm, &mut s) } == kResultOk)
                    .then(|| (norm, w_arr_to_string(&s)))
                    .filter(|(_, label)| !label.is_empty())
            })
            .collect();
        (count, points)
    }

    /// Unit label (e.g. "dB", "Hz") from the parameter's `units`.
    pub fn param_label(&self, index: u32) -> String {
        let Some(c) = &self.controller else {
            return String::new();
        };
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { c.getParameterInfo(index as int32, &mut info) } == kResultOk {
            w_arr_to_string(&info.units)
        } else {
            String::new()
        }
    }
    /// Formatted display of the current value (via `getParamStringByValue`).
    pub fn param_display(&self, index: u32) -> String {
        let (Some(c), Some(id)) = (self.controller.as_ref(), self.param_id(index)) else {
            return String::new();
        };
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
        let stream = ComWrapper::new(MemStream {
            inner: shared.clone(),
        });
        let Some(ptr) = stream.to_com_ptr::<IBStream>() else {
            return Vec::new();
        };
        let ok = unsafe { self.component.getState(ptr.as_ptr()) } == kResultOk;
        if !ok {
            return Vec::new();
        }
        let data = shared.lock().unwrap().0.clone();
        data
    }

    /// Restore component state from a blob produced by [`Self::get_state`]. Also
    /// pushes it to the edit controller so its parameter view syncs.
    pub fn set_state(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // Component state.
        let shared = Arc::new(std::sync::Mutex::new((data.to_vec(), 0usize)));
        let stream = ComWrapper::new(MemStream {
            inner: shared.clone(),
        });
        if let Some(ptr) = stream.to_com_ptr::<IBStream>() {
            unsafe {
                self.component.setState(ptr.as_ptr());
            }
        }
        // Mirror into the controller (rewind a fresh stream first).
        if let Some(c) = &self.controller {
            let s2 = ComWrapper::new(MemStream {
                inner: Arc::new(std::sync::Mutex::new((data.to_vec(), 0usize))),
            });
            if let Some(ptr) = s2.to_com_ptr::<IBStream>() {
                unsafe {
                    c.setComponentState(ptr.as_ptr());
                }
            }
        }
    }
}

impl Drop for Vst3RealInstance {
    fn drop(&mut self) {
        // Cut the editor thread loose first: past this it can no longer reach
        // the view (or the controller behind it) we are about to terminate.
        // Detaching is NOT done here: `removed()` on a view that was never
        // attached trips a hard assert in DPF plugins, and the editor thread
        // already calls `close()` on its way out. Releasing the view is enough.
        drop(
            self.shared_view
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(),
        );
        drop(
            self.shared_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(),
        );
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some(c) = &self.controller {
                c.terminate();
            }
            self.component.terminate();
        }
    }
}

/// Convert a fixed `char8` C array to a Rust String (NUL-terminated).
fn c_arr_to_string(buf: &[char8]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Convert a fixed UTF-16 `TChar` array (VST3 String128) to a Rust String.
pub fn w_arr_to_string(buf: &[TChar]) -> String {
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
    Some(FactoryInfo {
        name,
        vendor,
        is_instrument,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path a knob in the plugin's own window takes: `performEdit` on the
    /// host handler, then the block's input parameter changes. If this queue
    /// reports nothing, the GUI moves and the sound does not.
    #[test]
    fn a_gui_edit_reaches_the_next_process_block() {
        let feed = EditFeed::default();
        let handler = HostComponentHandler { feed: feed.clone() };
        // SAFETY: plain data; no plugin involved.
        unsafe {
            handler.performEdit(7, 0.25);
            handler.performEdit(9, 0.75);
        }

        let mut scratch = Vec::new();
        feed.drain(&mut scratch);
        assert_eq!(scratch, vec![(7, 0.25), (9, 0.75)]);

        let changes = HostParamChanges::new();
        changes.load(&scratch);
        // SAFETY: same, the COM methods here only touch our own fields.
        unsafe {
            assert_eq!(changes.getParameterCount(), 2);
            let queues = changes.queues.lock().unwrap();
            assert_eq!(queues[1].getParameterId(), 9);
            let (mut off, mut value) = (-1, 0.0);
            assert_eq!(queues[1].getPoint(0, &mut off, &mut value), kResultOk);
            assert_eq!((off, value), (0, 0.75));
        }

        // Draining leaves nothing behind: the next block must not re-apply it.
        feed.drain(&mut scratch);
        assert!(scratch.is_empty());
    }

    /// MIDI learn asks "what did the user just grab?". Reading consumes the
    /// answer, so an old touch cannot bind a later CC.
    #[test]
    fn the_last_touched_parameter_is_reported_once() {
        let feed = EditFeed::default();
        assert_eq!(feed.take_last_touched(), None);
        feed.push(3, 0.5);
        feed.push(4, 0.5);
        assert_eq!(feed.take_last_touched(), Some((4, 0.5)));
        assert_eq!(feed.take_last_touched(), None);
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    /// A null `processContext` is what choz used to hand every VST3 plugin: it
    /// means "the host does not know what time it is", and a tempo-synced delay
    /// falls back to whatever it guesses. This is the same clock the VST2 host
    /// answers with, in VST3's units.
    #[test]
    fn the_process_context_carries_the_host_clock() {
        let t = choz_ports::transport();
        t.set_sample_rate(48_000);
        t.set_bpm(90.0);
        t.set_playing(true);
        t.rewind();
        t.advance(48_000); // one second

        let ctx = host_process_context();
        assert_eq!(ctx.sampleRate, 48_000.0);
        assert_eq!(ctx.projectTimeSamples, 48_000);
        // One second at 90 BPM is a beat and a half.
        assert!(
            (ctx.projectTimeMusic - 1.5).abs() < 1e-9,
            "{}",
            ctx.projectTimeMusic
        );
        assert_eq!(ctx.tempo, 90.0);
        let flags = ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | ProcessContext_::StatesAndFlags_::kPlaying;
        assert_eq!(ctx.state & flags, flags);

        // The bar the playhead is in, and where that bar began. At 4/4 and
        // 1.5 quarters in, that is bar 1 starting at 0; five quarters in it is
        // bar 2 starting at 4.
        assert_eq!(
            ctx.state & ProcessContext_::StatesAndFlags_::kBarPositionValid,
            ProcessContext_::StatesAndFlags_::kBarPositionValid,
            "the bar position is offered, not withheld"
        );
        assert!(
            (ctx.barPositionMusic - 0.0).abs() < 1e-9,
            "{}",
            ctx.barPositionMusic
        );

        t.set_time_signature(6, 8);
        let ctx = host_process_context();
        assert_eq!((ctx.timeSigNumerator, ctx.timeSigDenominator), (6, 8));
        t.set_time_signature(4, 4);

        t.set_playing(false);
        assert_eq!(
            host_process_context().state & ProcessContext_::StatesAndFlags_::kPlaying,
            0,
            "stopped means stopped"
        );
        t.set_bpm(choz_ports::Transport::DEFAULT_BPM);
        t.rewind();
    }
}
