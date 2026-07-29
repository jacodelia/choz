//! Real CLAP hosting via the safe [`clack_host`] crate (behind `feature = clap`).
//!
//! Trimmed from seqterm's host: choz drives a single MIDI channel with no MPE,
//! per-note expression, or state persistence.

#![cfg(feature = "clap")]

use std::ffi::CString;
use std::path::Path;

use clack_host::utils::Cookie;
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::prelude::*;

use choz_ports::{AudioSource, FxProcessor};

use crate::ClapPluginInfo;

// ── Host handler (no host extensions; all callbacks are no-ops) ─────────────

struct ChozShared;
impl<'a> SharedHandler<'a> for ChozShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct ChozHost;
impl HostHandlers for ChozHost {
    type Shared<'a> = ChozShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

fn host_info() -> HostInfo {
    HostInfo::new("choz", "choz", "https://github.com/jorgecodelia/choz", "0.1.0")
        .expect("static host info has no interior nul")
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// Enumerate every plugin a `.clap` file exposes, with real metadata. Returns an
/// empty vec on any failure so a directory scan never aborts on one bad file.
pub fn read_descriptors(path: &Path) -> Vec<ClapPluginInfo> {
    // SAFETY: loading an external library is inherently unsafe; clack handles the
    // ABI. We only read descriptors and drop the entry immediately after.
    let Ok(entry) = (unsafe { PluginEntry::load(path) }) else {
        return Vec::new();
    };
    let Some(factory) = entry.get_plugin_factory() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for desc in factory.plugin_descriptors() {
        let id = desc.id().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let is_instrument = desc
            .features()
            .any(|f| f.to_string_lossy() == "instrument");
        out.push(ClapPluginInfo {
            path: path.to_path_buf(),
            name: desc.name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            vendor: desc.vendor().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            id,
            is_instrument,
        });
    }
    out
}

// ── Live instrument ─────────────────────────────────────────────────────────

/// Channels choz itself works in (interleaved stereo).
const CHANNELS: usize = 2;

/// Channel count of every audio port the plugin declares, per direction:
/// `(inputs, outputs)`. A plugin gets exactly the ports it asked for — passing a
/// stereo-shaped guess to a mono plugin, or one buffer to a plugin with a
/// sidechain port, makes it reject the layout outright (DPF plugins assert).
/// Defaults to a single stereo port when the extension is missing.
fn port_layout(instance: &mut PluginInstance<ChozHost>) -> (Vec<usize>, Vec<usize>) {
    use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};

    let mut handle = instance.plugin_handle();
    let Some(ports) = handle.get_extension::<PluginAudioPorts>() else {
        return (vec![CHANNELS], vec![CHANNELS]);
    };
    let mut buf = AudioPortInfoBuffer::new();
    let mut side = |is_input: bool, buf: &mut AudioPortInfoBuffer| -> Vec<usize> {
        (0..ports.count(&mut handle, is_input))
            .map(|i| {
                ports
                    .get(&mut handle, i, is_input, buf)
                    .map(|info| (info.channel_count as usize).max(1))
                    .unwrap_or(CHANNELS)
            })
            .collect()
    };
    let ins = side(true, &mut buf);
    let outs = side(false, &mut buf);
    (ins, outs)
}

/// Planar buffers for one direction: `[port][channel][frame]`.
fn alloc_ports(layout: &[usize], frames: usize) -> Vec<Vec<Vec<f32>>> {
    layout.iter().map(|ch| vec![vec![0.0; frames]; *ch]).collect()
}

/// An event queued from the audio thread, flushed into the plugin's input event
/// list on the next `process` call.
enum QueuedEvent {
    NoteOn { key: i16, note_id: i32, velocity: f64 },
    NoteOff { key: i16, note_id: i32 },
    Param { id: u32, value: f64 },
}

/// Tracks sounding voices so each note-off targets the right CLAP `note_id`.
/// Linear-scanned (voice counts are small) to avoid RT allocation.
#[derive(Default)]
struct NoteRegistry {
    next: u32,
    active: Vec<(u8, u32)>, // (key, note_id)
}

impl NoteRegistry {
    fn alloc(&mut self, key: u8) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.active.push((key, id));
        id
    }
    fn take(&mut self, key: u8) -> Option<u32> {
        self.active
            .iter()
            .position(|(k, _)| *k == key)
            .map(|pos| self.active.swap_remove(pos).1)
    }
}

/// Read a plugin's parameter list without activating it. Non-RT: it dlopens the
/// library and instantiates the plugin, so call it once when the FX is added,
/// not per block.
pub fn read_params(path: &Path, plugin_id: &str) -> Vec<crate::ClapParamInfo> {
    use clack_extensions::params::{ParamInfoBuffer, PluginParams};

    // SAFETY: external library load; clack handles the ABI.
    let Ok(entry) = (unsafe { PluginEntry::load(path) }) else { return Vec::new() };
    let Ok(id) = CString::new(plugin_id) else { return Vec::new() };
    let Ok(mut instance) =
        PluginInstance::<ChozHost>::new(|_| ChozShared, |_| (), &entry, id.as_c_str(), &host_info())
    else {
        return Vec::new();
    };

    let mut handle = instance.plugin_handle();
    let Some(params) = handle.get_extension::<PluginParams>() else { return Vec::new() };
    let count = params.count(&mut handle);
    let mut buf = ParamInfoBuffer::new();
    let mut out = Vec::new();
    for i in 0..count {
        let Some(info) = params.get_info(&mut handle, i, &mut buf) else { continue };
        // A parameter with min == max can't be moved; skip it.
        if info.max_value <= info.min_value {
            continue;
        }
        out.push(crate::ClapParamInfo {
            id: info.id.into(),
            name: String::from_utf8_lossy(info.name).trim_end_matches('\0').to_string(),
            min: info.min_value,
            max: info.max_value,
            default: info.default_value,
        });
    }
    out
}

/// An activated CLAP plugin plus its planar scratch buffers. Shared by the
/// instrument ([`ClapInstrument`]) and effect ([`ClapEffect`]) wrappers, which
/// differ only in what they put into `in_buf` and the event list.
struct ClapProc {
    // Field order matters for drop: processor, then instance, then the entry —
    // the entry owns the dlopen'd library, so letting it go first leaves the
    // instance pointing at unmapped code (a segfault on teardown).
    processor: Option<StartedPluginAudioProcessor<ChozHost>>,
    /// `Option` only so `Drop` can take them out and leak them; see there.
    instance: Option<PluginInstance<ChozHost>>,
    entry: Option<PluginEntry>,
    out_ports: AudioPorts,
    in_ports: AudioPorts,
    /// Planar audio, `[port][channel][frame]`. Only port 0 carries choz's
    /// signal; extra ports (sidechains) exist so the layout matches what the
    /// plugin declared, and stay silent.
    in_buf: Vec<Vec<Vec<f32>>>,
    out_buf: Vec<Vec<Vec<f32>>>,
    steady: u64,
    max_frames: u32,
}

// SAFETY: `PluginInstance` is `!Send` because CLAP main-thread callbacks must run
// on its creating thread. After construction we only call the (Send) audio
// processor's `process` and our own note queue, all on the single audio thread.
// The instance is kept alive only to own the processor and tear it down on drop.
unsafe impl Send for ClapProc {}

impl ClapProc {
    /// Load and activate a CLAP plugin. Returns `None` on any failure.
    fn build(path: &Path, plugin_id: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        let max_block = max_block.max(1);
        // SAFETY: external library load; clack handles the ABI.
        let entry = unsafe { PluginEntry::load(path) }.ok()?;
        let id = CString::new(plugin_id).ok()?;

        let mut instance = PluginInstance::<ChozHost>::new(
            |_| ChozShared,
            |_| (),
            &entry,
            id.as_c_str(),
            &host_info(),
        )
        .ok()?;

        let config = PluginAudioConfiguration {
            sample_rate: sample_rate as f64,
            min_frames_count: 1,
            max_frames_count: max_block,
        };
        let (in_layout, out_layout) = port_layout(&mut instance);
        let stopped = instance.activate(|_, _| (), config).ok()?;
        let started = stopped.start_processing().ok()?;

        let frames = max_block as usize;
        Some(Self {
            processor: Some(started),
            entry: Some(entry),
            in_ports: AudioPorts::with_capacity(in_layout.iter().sum(), in_layout.len().max(1)),
            out_ports: AudioPorts::with_capacity(out_layout.iter().sum(), out_layout.len().max(1)),
            in_buf: alloc_ports(&in_layout, frames),
            out_buf: alloc_ports(&out_layout, frames),
            instance: Some(instance),
            steady: 0,
            max_frames: max_block,
        })
    }

    /// Which output-buffer channels to read as left/right. A mono plugin feeds
    /// both sides from its single channel.
    fn out_channels(&self) -> (usize, usize) {
        let main = self.out_buf.first().map(|p| p.len()).unwrap_or(0);
        (0, if main > 1 { 1 } else { 0 })
    }

    /// The main (first) output port's planar buffers.
    fn main_out(&self) -> &[Vec<f32>] {
        self.out_buf.first().map(|p| p.as_slice()).unwrap_or(&[])
    }

    /// Run one block. `in_buf` must already hold the input audio; results land
    /// in `out_buf`. Does nothing if the processor failed to start.
    fn process_block(&mut self, frames: usize, queue: &[QueuedEvent]) {
        let Self { processor, in_ports, out_ports, in_buf, out_buf, steady, .. } = self;
        let Some(proc) = processor.as_mut() else { return };

        for port in out_buf.iter_mut() {
            for ch in port.iter_mut() {
                for v in ch[..frames].iter_mut() { *v = 0.0; }
            }
        }

        // Build the input event list (channel 0, port 0).
        let mut in_ev = EventBuffer::new();
        for q in queue.iter() {
            match q {
                QueuedEvent::NoteOn { key, note_id, velocity } => {
                    let pckn = Pckn::from_raw(0, 0, *key, *note_id);
                    in_ev.push(&NoteOnEvent::new(0, pckn, *velocity));
                }
                QueuedEvent::NoteOff { key, note_id } => {
                    let pckn = Pckn::from_raw(0, 0, *key, *note_id);
                    in_ev.push(&NoteOffEvent::new(0, pckn, 0.0));
                }
                QueuedEvent::Param { id, value } => {
                    if let Some(param_id) = ClapId::from_raw(*id) {
                        in_ev.push(&ParamValueEvent::new(
                            0, param_id, Pckn::match_all(), *value, Cookie::empty(),
                        ));
                    }
                }
            }
        }
        let input_events = InputEvents::from(&in_ev);
        let mut out_ev = EventBuffer::new();
        let mut output_events = OutputEvents::from(&mut out_ev);

        // One buffer per declared port, in the plugin's own order.
        let input_audio = in_ports.with_input_buffers(in_buf.iter_mut().map(|port| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(port.iter_mut().map(InputChannel::variable)),
        }));
        let mut output_audio = out_ports.with_output_buffers(out_buf.iter_mut().map(|port| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(port.iter_mut().map(|b| b.as_mut_slice())),
        }));

        let _ = proc.process(
            &input_audio,
            &mut output_audio,
            &input_events,
            &mut output_events,
            Some(*steady),
            None,
        );
        *steady += frames as u64;
    }
}

impl Drop for ClapProc {
    /// Tearing a plugin down is where third-party code most often takes the
    /// host with it: `ZaMaximX2` (DPF) segfaults inside its own `deactivate`,
    /// reproducibly, with a bare clack host and no processing at all. Losing a
    /// live session because the user removed an effect is worse than holding on
    /// to the instance, so by default choz stops processing and then leaks the
    /// plugin — the OS reclaims it at exit.
    ///
    /// Set `CHOZ_CLAP_STRICT_TEARDOWN=1` for the correct-but-trusting sequence
    /// (deactivate + destroy), e.g. when checking a plugin's behaviour or
    /// hunting a leak.
    fn drop(&mut self) {
        let strict = std::env::var_os("CHOZ_CLAP_STRICT_TEARDOWN").is_some();
        let stopped = self.processor.take().map(|started| started.stop_processing());

        if strict {
            if let (Some(stopped), Some(instance)) = (stopped, self.instance.as_mut()) {
                instance.deactivate(stopped);
            }
            return;
        }
        // Destroying a still-active plugin is undefined behaviour, so the
        // processor, the instance and the library entry all have to stay.
        std::mem::forget(stopped);
        std::mem::forget(self.instance.take());
        std::mem::forget(self.entry.take());
    }
}

/// A live CLAP instrument rendering interleaved stereo and accepting notes on a
/// single channel. Implements [`AudioSource`], so the engine plays it like any
/// other source and the existing note ring drives it.
pub struct ClapInstrument {
    proc: ClapProc,
    queue: Vec<QueuedEvent>,
    notes: NoteRegistry,
    /// The plugin's parameters, in the order the UI shows them. `set_param`
    /// indexes this list.
    params: Vec<crate::ClapParamInfo>,
}

impl ClapInstrument {
    /// Load and activate a CLAP instrument. Returns `None` on any failure.
    pub fn build(path: &Path, plugin_id: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        Some(Self {
            proc: ClapProc::build(path, plugin_id, sample_rate, max_block)?,
            queue: Vec::with_capacity(64),
            notes: NoteRegistry::default(),
            params: read_params(path, plugin_id),
        })
    }
}

/// Queue a normalised parameter change for the next block. RT-safe: nothing is
/// allocated, and a full queue drops the change instead of growing.
fn queue_param(queue: &mut Vec<QueuedEvent>, params: &[crate::ClapParamInfo], index: usize, value: f32) {
    let Some(info) = params.get(index) else { return };
    if queue.len() == queue.capacity() {
        return;
    }
    queue.push(QueuedEvent::Param { id: info.id, value: info.plain(value as f64) });
}

impl AudioSource for ClapInstrument {
    fn render(&mut self, output: &mut [f32], _sample_rate: u32) -> usize {
        let frames = (output.len() / 2).min(self.proc.max_frames as usize);
        for s in output.iter_mut() {
            *s = 0.0;
        }
        if frames == 0 {
            return output.len() / 2;
        }

        // An instrument takes no audio input; feed every port silence.
        for port in self.proc.in_buf.iter_mut() {
            for ch in port.iter_mut() {
                for v in ch[..frames].iter_mut() { *v = 0.0; }
            }
        }
        self.proc.process_block(frames, &self.queue);
        self.queue.clear();

        let (l, r) = self.proc.out_channels();
        let out = self.proc.main_out();
        if out.is_empty() {
            return frames;
        }
        for i in 0..frames {
            output[i * 2] = out[l][i];
            output[i * 2 + 1] = out[r][i];
        }
        frames
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        let note_id = self.notes.alloc(note);
        self.queue.push(QueuedEvent::NoteOn {
            key: note as i16,
            note_id: note_id as i32,
            velocity: velocity as f64 / 127.0,
        });
    }

    fn note_off(&mut self, note: u8) {
        let note_id = self.notes.take(note).map(|id| id as i32).unwrap_or(-1);
        self.queue.push(QueuedEvent::NoteOff { key: note as i16, note_id });
    }

    fn set_param(&mut self, index: usize, value: f32) {
        queue_param(&mut self.queue, &self.params, index, value);
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}

/// A live CLAP *audio effect* in a slot's FX chain. Implements [`FxProcessor`],
/// so it sits alongside the built-in FX. Blocks longer than the plugin's
/// configured maximum are processed in chunks.
pub struct ClapEffect {
    proc: ClapProc,
    wet: f32,
    /// The plugin's parameters, in the order the UI shows them. `set_param`
    /// indexes this list.
    params: Vec<crate::ClapParamInfo>,
    /// Param changes waiting to be handed to the plugin on the next block.
    queue: Vec<QueuedEvent>,
}

impl ClapEffect {
    pub fn build(path: &Path, plugin_id: &str, sample_rate: u32, max_block: u32) -> Option<Self> {
        Some(Self {
            proc: ClapProc::build(path, plugin_id, sample_rate, max_block)?,
            wet: 1.0,
            params: read_params(path, plugin_id),
            // Enough headroom for a burst of knob turns between two blocks.
            queue: Vec::with_capacity(64),
        })
    }
}

impl FxProcessor for ClapEffect {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let chunk = self.proc.max_frames as usize;
        for block in buf.chunks_mut(chunk * CHANNELS) {
            let frames = block.len() / CHANNELS;
            if frames == 0 {
                continue;
            }
            // The main input port gets the signal (mono plugins get the stereo
            // mid); sidechain ports stay silent.
            for (p, port) in self.proc.in_buf.iter_mut().enumerate() {
                let mono = port.len() == 1;
                for (ch, buf) in port.iter_mut().enumerate() {
                    for i in 0..frames {
                        buf[i] = match (p, mono) {
                            (0, true) => (block[i * 2] + block[i * 2 + 1]) * 0.5,
                            (0, false) => block[i * 2 + ch % 2],
                            _ => 0.0,
                        };
                    }
                }
            }
            self.proc.process_block(frames, &self.queue);
            self.queue.clear();
            let (l, r) = self.proc.out_channels();
            let dry = 1.0 - self.wet;
            let out = self.proc.main_out();
            if out.is_empty() {
                continue;
            }
            // A plugin that hands back NaN/Inf (ZamEQ2 does, before its
            // parameters are set) would blast the output device, so its
            // contribution is dropped rather than mixed in.
            for i in 0..frames {
                let (wl, wr) = (out[l][i], out[r][i]);
                let (wl, wr) = if wl.is_finite() && wr.is_finite() { (wl, wr) } else { (0.0, 0.0) };
                block[i * 2] = block[i * 2] * dry + wl * self.wet;
                block[i * 2 + 1] = block[i * 2 + 1] * dry + wr * self.wet;
            }
        }
    }

    fn reset(&mut self) {
        self.proc.steady = 0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn params(&self) -> Vec<choz_ports::FxParam> {
        // The trait's descriptor wants a 'static name, which a plugin's dynamic
        // names can't provide — the UI reads them with `read_params` instead.
        // What matters here is the count.
        self.params
            .iter()
            .map(|p| choz_ports::FxParam::new("param", 0.0, p.min as f32, p.max as f32, ""))
            .collect()
    }

    /// `index` is into the plugin's parameter list, `value` a 0..1 knob
    /// position. RT-safe: the event is queued, not sent, and the queue was
    /// pre-allocated (a full queue drops the change rather than allocating).
    fn set_param(&mut self, index: usize, value: f32) {
        queue_param(&mut self.queue, &self.params, index, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_registry_unique_ids_and_lookup() {
        let mut r = NoteRegistry::default();
        let a = r.alloc(60);
        let b = r.alloc(64);
        assert_ne!(a, b);
        assert_eq!(r.take(64), Some(b));
        assert_eq!(r.take(64), None);
        assert_eq!(r.take(60), Some(a));
        assert!(r.active.is_empty());
    }
}
