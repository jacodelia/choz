//! The plugin's own window, via `IPlugView` (VST3's editor interface).
//!
//! The shape is the one the other three formats already use: the live view is
//! shared with choz's editor thread through an `Arc<Mutex<Option<…>>>` that the
//! instance's `Drop` empties first, so a window still open when its slot is
//! replaced degrades to no-ops instead of calling freed COM objects.
//!
//! What is specific to VST3 on Linux is the **run loop**. A VST3 plugin does not
//! get an idle callback: it registers timers and file descriptors on the host's
//! `Steinberg::Linux::IRunLoop`, which it obtains by querying the `IPlugFrame`
//! the host handed it. JUCE-based plugins refuse to attach at all without one.
//! [`HostFrame`] is that frame *and* that run loop; [`Vst3Editor::idle`] is what
//! actually fires the registered handlers, and it is called from the editor
//! thread every ~30 ms.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use choz_ports::PluginEditor;
use libloading::Library;
use vst3::Steinberg::Linux::{
    FileDescriptor, IEventHandler, IEventHandlerTrait, IRunLoopTrait, ITimerHandler,
    ITimerHandlerTrait, TimerInterval,
};
use vst3::Steinberg::{
    IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewTrait, ViewRect, kPlatformTypeX11EmbedWindowID,
    kResultOk,
};
use vst3::{Class, ComPtr, ComRef, ComWrapper};

/// The live view plus what has to outlive it, shared with the editor thread.
/// `None` once the instance is gone.
pub type SharedView = Arc<Mutex<Option<ViewCell>>>;

pub struct ViewCell {
    pub view: ComPtr<IPlugView>,
    /// The frame handed to the plugin: it is also the run loop, so it must stay
    /// alive as long as the plugin can call back into it.
    pub frame: ComWrapper<HostFrame>,
    /// Keeps the bundle mapped while the editor thread can still call in.
    pub _lib: Arc<Library>,
}

// SAFETY: the view is only touched under the mutex, from the editor thread
// while the owning instance is alive (which is exactly what `Some` marks).
unsafe impl Send for ViewCell {}

// ─── The host's frame / run loop ────────────────────────────────────────────

struct Timer {
    handler: *mut ITimerHandler,
    interval: Duration,
    last: Instant,
}

struct Fd {
    handler: *mut IEventHandler,
    fd: FileDescriptor,
}

/// What the plugin registered on the run loop. The pointers belong to the
/// plugin and are only used between register and unregister.
#[derive(Default)]
struct RunLoopState {
    timers: Vec<Timer>,
    fds: Vec<Fd>,
}

/// `IPlugFrame` + `Steinberg::Linux::IRunLoop` in one object, which is how a
/// VST3 plugin finds the run loop: it queries its frame for it.
pub struct HostFrame {
    state: Mutex<RunLoopState>,
}

impl Class for HostFrame {
    type Interfaces = (IPlugFrame, vst3::Steinberg::Linux::IRunLoop);
}

impl HostFrame {
    fn new() -> Self {
        Self { state: Mutex::new(RunLoopState::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunLoopState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fire every handler that is due. Runs on the editor thread.
    fn tick(&self) {
        // The handlers are collected under the lock and called outside it: a
        // plugin is free to (un)register a timer from inside `onTimer`, and
        // holding the lock across that call would deadlock.
        let now = Instant::now();
        let (timers, fds) = {
            let mut st = self.lock();
            let due: Vec<*mut ITimerHandler> = st
                .timers
                .iter_mut()
                .filter(|t| now.duration_since(t.last) >= t.interval)
                .map(|t| {
                    t.last = now;
                    t.handler
                })
                .collect();
            let fds: Vec<(*mut IEventHandler, FileDescriptor)> =
                st.fds.iter().map(|f| (f.handler, f.fd)).collect();
            (due, fds)
        };
        // NOTE: `ComRef`, not `ComPtr` — a `ComPtr` takes ownership of the
        // reference and releases it when the temporary drops, which silently
        // over-releases a handler the plugin still owns (DPF noticed: "Host run
        // loop did not give away timer (refcount -29)").
        for handler in timers {
            // SAFETY: registered by the plugin and not yet unregistered; the
            // plugin owns the object and outlives the window.
            unsafe { ComRef::from_raw_unchecked(handler).onTimer() };
        }
        // Every registered descriptor is offered on every tick rather than only
        // when `poll` says it is readable. An X11 client reads events into its
        // own buffer, so the descriptor can be quiet while events are still
        // queued inside the toolkit — gating on `poll` leaves a window that
        // stops responding until the next byte arrives. The handlers drain and
        // return; that is what the run loop contract asks of them.
        for (handler, fd) in fds {
            // SAFETY: as above.
            unsafe { ComRef::from_raw_unchecked(handler).onFDIsSet(fd) };
        }
    }
}

impl IPlugFrameTrait for HostFrame {
    /// The plugin wants a different size. choz's window keeps the size the
    /// plugin asked for at open time, so this only tells the view its request
    /// was accepted.
    ///
    /// ponytail: resizing the X11 window from here needs a channel back to the
    /// editor thread; add it if a plugin turns out to need to grow after open.
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> i32 {
        if !view.is_null() && !new_size.is_null() {
            // SAFETY: both pointers come from the plugin and are valid for this
            // call.
            unsafe { ComRef::from_raw_unchecked(view).onSize(new_size) };
        }
        kResultOk
    }
}

impl IRunLoopTrait for HostFrame {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> i32 {
        if handler.is_null() {
            return vst3::Steinberg::kInvalidArgument;
        }
        self.lock().fds.push(Fd { handler, fd });
        kResultOk
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> i32 {
        self.lock().fds.retain(|f| f.handler != handler);
        kResultOk
    }

    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: TimerInterval) -> i32 {
        if handler.is_null() {
            return vst3::Steinberg::kInvalidArgument;
        }
        self.lock().timers.push(Timer {
            handler,
            // A plugin asking for 0 ms means "as often as you can"; the editor
            // thread's own 30 ms sleep is the real floor either way.
            interval: Duration::from_millis(milliseconds.max(1)),
            last: Instant::now(),
        });
        kResultOk
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> i32 {
        self.lock().timers.retain(|t| t.handler != handler);
        kResultOk
    }
}

// ─── The editor handle choz-ui drives ───────────────────────────────────────

pub struct Vst3Editor {
    shared: SharedView,
    /// Whether the view is currently attached. Open and close never overlap.
    attached: Mutex<bool>,
}

impl Vst3Editor {
    /// Build the shared cell for a freshly created view, or `None` when the
    /// plugin has no editor that can embed into an X11 window — which is what
    /// keeps the `GUI` button off a slot where it would do nothing.
    pub fn cell(view: ComPtr<IPlugView>, lib: Arc<Library>) -> Option<ViewCell> {
        // SAFETY: the view was just created by the plugin's controller.
        if unsafe { view.isPlatformTypeSupported(kPlatformTypeX11EmbedWindowID) } != kResultOk {
            return None;
        }
        Some(ViewCell { view, frame: ComWrapper::new(HostFrame::new()), _lib: lib })
    }

    pub fn new(shared: SharedView) -> Arc<Self> {
        Arc::new(Self { shared, attached: Mutex::new(false) })
    }
}

impl PluginEditor for Vst3Editor {
    fn open(&self, parent: u64) -> Option<(u16, u16)> {
        let mut attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
        if *attached {
            return None;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let cell = guard.as_ref()?;
        let frame = cell.frame.to_com_ptr::<IPlugFrame>()?;

        // SAFETY: the cell is `Some` only while the instance lives, and every
        // call below happens on this one thread under the locks held here.
        unsafe {
            // The frame goes in before attaching: it is where the plugin looks
            // for the run loop, and a JUCE editor that can't find one refuses.
            cell.view.setFrame(frame.as_ptr());
            if cell.view.attached(parent as usize as *mut c_void, kPlatformTypeX11EmbedWindowID)
                != kResultOk
            {
                eprintln!("choz: VST3 IPlugView::attached(X11) refused");
                cell.view.setFrame(std::ptr::null_mut());
                return None;
            }
            *attached = true;

            let mut r: ViewRect = std::mem::zeroed();
            (cell.view.getSize(&mut r) == kResultOk)
                .then(|| {
                    (
                        (r.right - r.left).clamp(0, u16::MAX as i32) as u16,
                        (r.bottom - r.top).clamp(0, u16::MAX as i32) as u16,
                    )
                })
                .filter(|(w, h)| *w > 0 && *h > 0)
        }
    }

    /// Fire the timers and descriptors the plugin registered on the run loop.
    /// This is what makes a VST3 window paint; there is no idle opcode.
    fn idle(&self) {
        let attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
        if !*attached {
            return;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        cell.frame.tick();
    }

    fn close(&self) {
        let mut attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
        if !*attached {
            return;
        }
        *attached = false;
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: attached exactly once by this type, detached exactly once —
        // the flag is cleared above before anything can re-enter.
        unsafe {
            cell.view.removed();
            cell.view.setFrame(std::ptr::null_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The run loop must survive being ticked with nothing registered, and a
    /// timer must not fire before its interval elapses — the two cases every
    /// idle call hits.
    #[test]
    fn timers_fire_no_sooner_than_their_interval() {
        let frame = HostFrame::new();
        frame.tick();

        // A handler pointer is never dereferenced while the interval is unmet,
        // so a dangling one is safe here and keeps the test free of COM setup.
        frame.lock().timers.push(Timer {
            handler: std::ptr::dangling_mut(),
            interval: Duration::from_secs(3600),
            last: Instant::now(),
        });
        frame.tick();
        assert_eq!(frame.lock().timers.len(), 1);
    }

}
