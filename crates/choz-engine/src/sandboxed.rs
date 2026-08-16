//! A plugin playing in a process of its own.
//!
//! [`choz_plugin_sandbox`] moves one block of audio across the boundary with a
//! deadline; this is the pair of ends that use it. The host end is an ordinary
//! [`AudioSource`], so a rack slot cannot tell the difference — except that
//! when the plugin dies, the slot goes quiet instead of the app going away.
//!
//! The child is the choz binary again, the same trick the scan and probe
//! workers use: [`sandbox_worker_main`] runs before anything touches a terminal
//! or an audio device.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use choz_plugin_sandbox::shm::Shm;
use choz_plugin_sandbox::{region_bytes, Host, Sandbox};

use crate::paths::PluginFormat;
use crate::sources::AudioSource;

/// Argument that turns the choz binary into a plugin sandbox.
pub const SANDBOX_WORKER_FLAG: &str = "--choz-sandbox-worker";

/// Interleaved stereo, like every other source in choz.
const CHANNELS: u32 = 2;

/// How long the audio thread will wait for the child before writing silence,
/// as a fraction of the block it is rendering. Two thirds leaves room for the
/// rest of the callback — the other slots still have to be mixed.
const DEADLINE_SHARE: f64 = 2.0 / 3.0;

/// Everything needed to start the child again after it dies.
#[derive(Clone)]
struct ChildSpec {
    exe: PathBuf,
    format: PluginFormat,
    path: PathBuf,
    id: String,
    shm_name: String,
    frames: u32,
}

impl ChildSpec {
    fn spawn(&self) -> Result<std::process::Child> {
        if self.format == PluginFormat::Pd {
            return self.spawn_pd();
        }
        std::process::Command::new(&self.exe)
            .env(crate::WORKER_ENV, "1")
            .arg(SANDBOX_WORKER_FLAG)
            .arg(self.format.label())
            .arg(&self.path)
            .arg(&self.id)
            .arg(&self.shm_name)
            .arg(self.frames.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .context("cannot start the plugin sandbox")
    }

    /// A Pure Data patch is served by `choz-pd-host`, not by choz itself: that
    /// binary is the one that links libpd, and choz deliberately does not.
    ///
    /// Looked for next to the choz binary first — that is where a build and an
    /// install both put it — then on `$PATH`. `CHOZ_PD_HOST` overrides both.
    fn spawn_pd(&self) -> Result<std::process::Child> {
        let exe = pd_host_exe();
        std::process::Command::new(&exe)
            .arg(&self.path)
            .arg(&self.shm_name)
            .arg(self.frames.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!(
                    "cannot start {} — Pure Data patches need it, and it is built with \
                     `cargo build -p choz-plugin-pd --features pd` (libpd installed)",
                    exe.display()
                )
            })
    }
}

/// Where the Pure Data child lives.
fn pd_host_exe() -> PathBuf {
    const EXE: &str = "choz-pd-host";
    if let Some(explicit) = std::env::var_os("CHOZ_PD_HOST") {
        return PathBuf::from(explicit);
    }
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(EXE)))
        .filter(|p| p.is_file())
    {
        return beside;
    }
    PathBuf::from(EXE)
}

/// The host end: a plugin instance living in another process.
pub struct SandboxedPlugin {
    bridge: Host,
    /// Kept for its mapping; dropping it unmaps the region. The name is only
    /// unlinked at the very end, because a replacement child has to be able to
    /// open it again. In an `Arc` because the window handle held by the UI
    /// thread keeps the same mapping alive.
    shm: Arc<Shm>,
    /// The live child, shared with the supervisor thread that replaces it.
    child: Arc<std::sync::Mutex<std::process::Child>>,
    /// Set on drop so the supervisor stops resurrecting anything.
    closing: Arc<AtomicBool>,
    /// Missed blocks and restarts, shared with whoever is showing them. The
    /// instance itself belongs to the RT thread, so this is all the UI gets.
    status: choz_ports::SandboxStatus,
    supervisor: Option<std::thread::JoinHandle<()>>,
    frames: usize,
    /// What an instrument gets as input. Allocated once: `render` must not.
    silence: Vec<f32>,
    /// Scratch for the input side of an in-place block.
    tail: Vec<f32>,
    /// Where the child's answer lands before it is copied out.
    answer: Vec<f32>,
    deadline: Duration,
}

impl SandboxedPlugin {
    /// Start `path` in its own process and wait for it to answer one block.
    ///
    /// Fails if the child can't be spawned or never answers, which is the same
    /// outcome as a plugin that refuses to instantiate: the caller reports it
    /// and the slot stays empty.
    pub fn build(
        format: PluginFormat,
        path: &Path,
        id: &str,
        sample_rate: u32,
        frames: u32,
    ) -> Result<Self> {
        let exe = std::env::current_exe().context("cannot find the choz binary")?;
        let name = choz_plugin_sandbox::shm::unique_name(&format!(
            "sbx-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let shm = Shm::create(&name, region_bytes(frames, CHANNELS))
            .context("cannot create the shared audio region")?;
        // SAFETY: the region is ours, freshly sized by `region_bytes`.
        let bridge = unsafe { Host::create(shm.as_ptr(), frames, CHANNELS, sample_rate) };

        let spec = ChildSpec {
            exe,
            format,
            path: path.to_path_buf(),
            id: id.to_string(),
            shm_name: name,
            frames,
        };
        let child = Arc::new(std::sync::Mutex::new(spec.spawn()?));

        let block = frames as usize * CHANNELS as usize;
        let mut me = Self {
            bridge,
            shm: Arc::new(shm),
            child: Arc::clone(&child),
            closing: Arc::new(AtomicBool::new(false)),
            status: choz_ports::SandboxStatus::default(),
            supervisor: None,
            frames: frames as usize,
            silence: vec![0.0; block],
            tail: vec![0.0; block],
            answer: vec![0.0; block],
            deadline: Duration::from_secs_f64(
                frames as f64 / sample_rate.max(1) as f64 * DEADLINE_SHARE,
            ),
        };

        // Loading a plugin takes as long as it takes (Surge XT is not quick);
        // only after the first answer is the deadline realtime-sized.
        let mut first = vec![0.0f32; block];
        let silence = vec![0.0f32; block];
        if !me
            .bridge
            .exchange(&silence, &mut first, Duration::from_secs(10))
        {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
            anyhow::bail!("{} never started in its sandbox", path.display());
        }
        me.supervise(spec);
        Ok(me)
    }

    /// Blocks the child failed to answer in time. Each one is silence the user
    /// heard.
    pub fn missed(&self) -> u64 {
        self.bridge.missed()
    }

    /// How many times the plugin has been restarted after crashing.
    pub fn restarts(&self) -> u64 {
        self.status.restarts()
    }

    /// The shared counters, for whoever draws them.
    pub fn status(&self) -> choz_ports::SandboxStatus {
        self.status.clone()
    }

    /// A `GUI` button for a plugin that lives in another process — only when
    /// the child reported that its plugin actually has a window. It answers
    /// that before serving its first block, so by the time `build` returns the
    /// answer is in.
    pub fn editor_handle(&self) -> Option<choz_ports::EditorHandle> {
        self.bridge
            .has_editor()
            .unwrap_or(false)
            .then(|| self.raw_editor_handle())
    }

    fn raw_editor_handle(&self) -> choz_ports::EditorHandle {
        // SAFETY: the link holds a clone of the mapping, so the region outlives
        // it however the instance ends.
        let link = unsafe { self.bridge.editor_link() };
        std::sync::Arc::new(SandboxEditor {
            link,
            _shm: Arc::clone(&self.shm),
        }) as choz_ports::EditorHandle
    }

    /// Republish the bridge's missed count. Called at the end of every block:
    /// a relaxed store is fine on the audio thread.
    fn publish(&self) {
        self.status
            .missed
            .store(self.bridge.missed(), Ordering::Relaxed);
    }

    /// The pid of the process currently hosting the plugin.
    pub fn child_pid(&self) -> u32 {
        self.child.lock().map(|c| c.id()).unwrap_or(0)
    }

    /// Watch the child and start a new one when it dies.
    ///
    /// This is why a sandboxed plugin is more than a crash shield: a plugin
    /// that segfaults comes *back*, a fraction of a second later, instead of
    /// leaving the tab silent until the user reloads it. The audio thread is
    /// not involved — it just reads silence from `exchange` in the meantime.
    fn supervise(&mut self, spec: ChildSpec) {
        let child = Arc::clone(&self.child);
        let closing = Arc::clone(&self.closing);
        let restarts = Arc::clone(&self.status.restarts);
        let handle = std::thread::Builder::new()
            .name("choz-sandbox-supervisor".into())
            .spawn(move || {
                loop {
                    // `wait` needs the lock only while the child is gone; poll
                    // instead so `Drop` can still kill it.
                    let exited = loop {
                        if closing.load(Ordering::Relaxed) {
                            return;
                        }
                        match child.lock().map(|mut c| c.try_wait()) {
                            Ok(Ok(Some(status))) => break status,
                            Ok(Ok(None)) => std::thread::sleep(Duration::from_millis(20)),
                            // Poisoned or unwaitable: nothing sensible left to do.
                            _ => return,
                        }
                    };
                    if closing.load(Ordering::Relaxed) {
                        return;
                    }
                    eprintln!("choz: plugin sandbox died ({exited}); restarting it");
                    match spec.spawn() {
                        Ok(fresh) => {
                            if let Ok(mut slot) = child.lock() {
                                *slot = fresh;
                            }
                            restarts.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            eprintln!("choz: cannot restart the plugin sandbox: {e}");
                            return;
                        }
                    }
                }
            })
            .ok();
        self.supervisor = handle;
    }
}

impl SandboxedPlugin {
    /// One round trip per block, in place. `buf` carries the input over and
    /// comes back holding the plugin's answer — silence for the blocks the
    /// child failed to deliver.
    ///
    /// Realtime-safe: the wait is bounded and nothing here allocates.
    fn exchange_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let block = self.frames * CHANNELS as usize;
        let mut done = 0;
        while done < buf.len() {
            let take = (buf.len() - done).min(block);
            // The input goes through `tail` and the answer lands in `answer`:
            // the caller's buffer is both source and destination, and the two
            // sides of the region must not alias.
            self.tail[..take].copy_from_slice(&buf[done..done + take]);
            self.tail[take..].fill(0.0);
            self.bridge
                .exchange(&self.tail, &mut self.answer, self.deadline);
            buf[done..done + take].copy_from_slice(&self.answer[..take]);
            done += take;
        }
        self.publish();
        // A sandboxed plugin is one choz already knows to be badly behaved, so
        // its output doesn't get to poison the mix either: padthv1 hands back
        // NaN until it has a patch loaded.
        for s in buf.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
            }
        }
    }
}

/// The `GUI` button of a plugin that lives in another process.
///
/// Nothing is opened here: the request crosses the shared region and the child
/// opens the window itself, in its own process, embedded into the X11 window
/// choz created. A GUI that crashes therefore kills only the child — which the
/// supervisor replaces — instead of the whole app.
struct SandboxEditor {
    link: choz_plugin_sandbox::bridge::EditorLink,
    /// Keeps the shared mapping alive while the window can still be asked for.
    _shm: Arc<Shm>,
}

impl choz_ports::PluginEditor for SandboxEditor {
    fn open(&self, parent: u64) -> Option<(u16, u16)> {
        // Generous: a big synth building its UI for the first time takes a
        // while, and this runs on the editor thread, never on audio.
        self.link.editor(Some(parent), Duration::from_secs(5))
    }

    /// Nothing to pump from here — the child idles its own window on its own
    /// thread, which is the only place the toolkit can be touched.
    fn idle(&self) {}

    fn close(&self) {
        self.link.editor(None, Duration::from_secs(2));
    }
}

impl AudioSource for SandboxedPlugin {
    fn render(&mut self, out: &mut [f32], _sample_rate: u32) -> usize {
        let block = self.frames * CHANNELS as usize;
        let mut done = 0;
        while done < out.len() {
            let take = (out.len() - done).min(block);
            if take == block {
                self.bridge
                    .exchange(&self.silence, &mut out[done..done + block], self.deadline);
            } else {
                // A partial tail: the child only ever processes whole blocks,
                // so it answers into `answer` and we keep what fits.
                self.bridge
                    .exchange(&self.silence, &mut self.answer, self.deadline);
                out[done..done + take].copy_from_slice(&self.answer[..take]);
            }
            done += take;
        }
        self.publish();
        // A sandboxed plugin is one choz already knows to be badly behaved, so
        // its output doesn't get to poison the mix either: padthv1 hands back
        // NaN until it has a patch loaded.
        for s in out.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
            }
        }
        out.len() / CHANNELS as usize
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.bridge.push_midi([0x90, note & 0x7F, velocity & 0x7F]);
    }

    fn note_off(&mut self, note: u8) {
        self.bridge.push_midi([0x80, note & 0x7F, 0]);
    }

    fn control_change(&mut self, cc: u8, value: u8) {
        self.bridge.push_midi([0xB0, cc & 0x7F, value & 0x7F]);
    }

    fn pitch_bend(&mut self, value: u16) {
        let v = value.min(16383);
        self.bridge
            .push_midi([0xE0, (v & 0x7F) as u8, (v >> 7) as u8]);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.bridge.push_param(index, value);
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }

    fn sandbox(&self) -> Option<choz_ports::SandboxStatus> {
        Some(self.status())
    }

    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.editor_handle()
    }
}

/// The same plugin, wired into an FX chain instead of a rack slot.
///
/// Everything that matters lives in [`SandboxedPlugin`]; this only swaps
/// silence for the dry signal and mixes the answer back in. Wet/dry is choz's
/// own, applied here — the child never sees it.
pub struct SandboxedEffect {
    inner: SandboxedPlugin,
    wet: f32,
    /// The dry block, kept so it can be mixed back after the round trip.
    dry: Vec<f32>,
}

impl SandboxedEffect {
    pub fn build(
        format: PluginFormat,
        path: &Path,
        id: &str,
        sample_rate: u32,
        frames: u32,
    ) -> Result<Self> {
        let inner = SandboxedPlugin::build(format, path, id, sample_rate, frames)?;
        Ok(Self {
            inner,
            wet: 1.0,
            dry: vec![0.0; frames as usize * CHANNELS as usize],
        })
    }
}

impl crate::fx::FxProcessor for SandboxedEffect {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.dry.len() < buf.len() {
            // A block bigger than the region: the round trip chunks it, but the
            // dry copy has to be able to hold it. Non-RT growth, once.
            self.dry.resize(buf.len(), 0.0);
        }
        self.dry[..buf.len()].copy_from_slice(buf);
        self.inner.exchange_block(buf, sample_rate);
        if self.wet < 1.0 {
            for (out, dry) in buf.iter_mut().zip(&self.dry) {
                *out = *dry * (1.0 - self.wet) + *out * self.wet;
            }
        }
    }

    fn reset(&mut self) {}

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "sandboxed plugin"
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inner.bridge.push_param(index, value);
    }

    fn sandbox(&self) -> Option<choz_ports::SandboxStatus> {
        Some(self.inner.status())
    }

    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.inner.editor_handle()
    }
}

impl Drop for SandboxedPlugin {
    fn drop(&mut self) {
        // Order matters: tell the supervisor to stand down *before* the child
        // dies, or it will helpfully start another one.
        self.closing.store(true, Ordering::Relaxed);
        self.bridge.stop();
        if let Some(t) = self.supervisor.take() {
            let _ = t.join();
        }
        if let Ok(mut child) = self.child.lock() {
            // It exits on its own once it sees `quit`; kill it if it has
            // stopped listening — which is exactly the case a sandbox is for.
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ─── The child ──────────────────────────────────────────────────────────────

/// Load the named plugin and answer blocks until the host says stop. Returns
/// `false` when the arguments aren't a sandbox invocation.
///
/// Call it from `main` beside the scan and probe workers.
pub fn sandbox_worker_main() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 || args[1] != SANDBOX_WORKER_FLAG {
        return false;
    }
    let Some(format) = PluginFormat::from_label(&args[2]) else {
        return true;
    };
    let (path, id, name) = (Path::new(&args[3]), args[4].as_str(), args[5].as_str());
    let frames: u32 = args[6].parse().unwrap_or(256);

    if let Err(e) = serve_plugin(format, path, id, name, frames) {
        eprintln!("choz: sandbox for {}: {e}", path.display());
    }
    true
}

fn serve_plugin(
    format: PluginFormat,
    path: &Path,
    id: &str,
    shm_name: &str,
    frames: u32,
) -> Result<()> {
    // In here, a UI that segfaults costs a child process the supervisor will
    // replace — which is exactly what the deny-list exists to avoid in choz's
    // own process. So the plugins whose editors are refused there get theirs
    // back here.
    choz_plugin_lv2::allow_denied_uis(true);

    let shm = Shm::attach(shm_name, region_bytes(frames, CHANNELS))
        .context("cannot attach the shared audio region")?;
    // SAFETY: same size and layout the host created.
    let mut sandbox = unsafe { Sandbox::attach(shm.as_ptr(), frames, CHANNELS) };
    let sample_rate = sandbox.sample_rate();

    // An instrument if it can be one, an effect otherwise — the same order the
    // load probe uses.
    let mut source = crate::engine::build_instrument(format, path, id, sample_rate, frames).ok();
    let mut effect = if source.is_none() {
        crate::fx_chain::build_plugin_fx_in_process(
            &crate::fx_chain::PluginFxRef {
                format,
                path: path.to_path_buf(),
                id: id.to_string(),
            },
            sample_rate,
            frames,
        )
    } else {
        None
    };
    if source.is_none() && effect.is_none() {
        anyhow::bail!("nothing loadable at {}", path.display());
    }

    // The plugin's window, opened **here**, in the child. That is the whole
    // point: a GUI that segfaults (every guitarix UI does) takes this process
    // down and the supervisor starts another, instead of taking choz with it.
    // The X11 window it embeds into belongs to choz — window ids are valid
    // across processes.
    let editor = source
        .as_ref()
        .and_then(|s| s.editor())
        .or_else(|| effect.as_ref().and_then(|f| f.editor()));
    // Say so before the first block: the host captures the editor handle as
    // soon as `build` returns, and it has no other way to know.
    sandbox.set_editor_present(editor.is_some());
    let mut window: Option<EditorThread> = None;
    let (size_tx, size_rx) = std::sync::mpsc::channel::<(u32, Option<(u16, u16)>)>();

    // The host waits with a deadline, so a slow block costs it silence, not a
    // stall. Ours is generous: it only bounds "the host went away".
    while sandbox.serve(
        Duration::from_secs(5),
        &mut |input, output, midi, params| {
            if let Some(src) = source.as_mut() {
                for (index, value) in params {
                    src.set_param(*index, *value);
                }
                for m in midi {
                    match m[0] & 0xF0 {
                        0x90 if m[2] > 0 => src.note_on(m[1], m[2]),
                        0x90 | 0x80 => src.note_off(m[1]),
                        0xB0 => src.control_change(m[1], m[2]),
                        0xE0 => src.pitch_bend(u16::from(m[1]) | u16::from(m[2]) << 7),
                        _ => {}
                    }
                }
                src.render(output, sample_rate);
            } else if let Some(fx) = effect.as_mut() {
                for (index, value) in params {
                    fx.set_param(*index, *value);
                }
                output.copy_from_slice(input);
                fx.process_block(output, sample_rate);
            }
        },
    ) {
        // Between blocks: the window requests. Opening one can take hundreds of
        // milliseconds, so it happens on its own thread and the answer comes
        // back through the channel — the audio rendezvous never waits for a
        // toolkit.
        if let Some((seq, parent)) = sandbox.editor_request() {
            match (parent, editor.clone()) {
                (Some(parent), Some(handle)) => {
                    window = Some(EditorThread::start(handle, parent, seq, size_tx.clone()));
                }
                _ => {
                    // Close, or nothing to open: answer at once.
                    window.take();
                    sandbox.editor_done(seq, None);
                }
            }
        }
        while let Ok((seq, size)) = size_rx.try_recv() {
            sandbox.editor_done(seq, size);
        }
    }
    Ok(())
}

/// The thread that owns the plugin's window inside the child.
struct EditorThread {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl EditorThread {
    fn start(
        handle: choz_ports::EditorHandle,
        parent: u64,
        seq: u32,
        tx: std::sync::mpsc::Sender<(u32, Option<(u16, u16)>)>,
    ) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("choz-sandbox-editor".into())
            .spawn(move || {
                let size = handle.open(parent);
                let _ = tx.send((seq, size));
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    handle.idle();
                    std::thread::sleep(Duration::from_millis(30));
                }
                handle.close();
            })
            .ok();
        Self { stop, join }
    }
}

impl Drop for EditorThread {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
