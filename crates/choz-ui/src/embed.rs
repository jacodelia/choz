//! choz inside somebody else's host: the same rack and the same panels, with
//! the DAW holding the clock.
//!
//! A plugin is choz with three things taken away — the sound card, the
//! terminal, and the command line — and nothing else changed. What is left is
//! [`Embedded`]: the [`App`](crate::App) the interface already drives, a
//! [`choz_engine::EmbeddedRt`] for the host's audio thread, and the four verbs
//! a plugin needs to reach them.
//!
//! **Two threads, and the split is the one choz already has.** The rack is
//! edited from wherever the interface runs and rendered wherever the host says
//! — exactly the division a sound card imposes, so nothing here is new: the
//! command ring between them is the same ring cpal and JACK use. [`Embedded`]
//! is the interface half and stays on the host's main thread; the `EmbeddedRt`
//! goes to the audio thread and never comes back.

use anyhow::Result;
use ratatui::{backend::Backend, Terminal};

use crate::{ui, App};

/// Sixteen stereo pairs, which is what a sampler with individual outs gives a
/// host and what the rack can fill: one tab per pair, routed with
/// `set_slot_out`. The host sees sixteen output ports and puts each on its own
/// track.
pub const PAIRS: usize = 16;

/// The interface half of an embedded choz.
pub struct Embedded {
    app: App,
    /// Running Bank Select MSB for the host's note port, kept for the same
    /// reason every other port keeps one: a program change arrives after its
    /// bank and has to travel with it.
    bank: u8,
}

impl Embedded {
    /// Build a rack with no device under it. `max_frames` is the largest block
    /// the host will ever ask for.
    ///
    /// `None` when the engine will not start, which embedded means the command
    /// ring was already taken — there is no card here to fail.
    pub fn new(
        sample_rate: u32,
        max_frames: u32,
        inputs: usize,
    ) -> Option<(Self, choz_engine::EmbeddedRt)> {
        // Before anything scans: three things in the engine re-run
        // `current_exe` to get a worker, and here that is the DAW.
        choz_engine::set_embedded();
        let mut app = App::new();
        // Armed before a single block is rendered: the run-away it exists for
        // can happen on the first one. Same as the standalone splash path.
        choz_engine::feedback::arm(app.ui.audio.feedback_guard);
        let mut engine = choz_engine::AudioEngine::new(sample_rate, max_frames);
        let rt = engine.start_embedded(PAIRS * 2, inputs, max_frames).ok()?;
        app.audio_engine = Some(engine);
        // No splash: a plugin window that spends three seconds on a logo before
        // it will answer a key is a plugin window nobody opens twice.
        app.splash_done = true;
        app.connect_midi();
        app.discover_synths(false);
        app.refresh_in_ports();
        app.saved_project = Some(app.project_snapshot());
        Some((Self { app, bank: 0 }, rt))
    }

    /// One turn of the interface loop: everything `run_app` does between two
    /// frames, minus the frame and the terminal's events.
    ///
    /// It is the arpeggiator's clock as much as the panel's refresh, so the
    /// host has to call it steadily — a plugin that only ticks when a key is
    /// pressed is a plugin whose patterns stutter.
    pub fn tick(&mut self) {
        let app = &mut self.app;
        app.run_pending_load();
        app.poll_scan();
        app.poll_midi_hotplug();
        app.poll_jack_midi();
        app.drain_midi();
        app.tick_arps();
        app.tick_seqs();
        app.pump_loopers();
        app.tick_notes();
        app.publish_chord();
        app.poll_editor();
        app.poll_preset_list();
        app.poll_instr_readback();
        app.poll_preset_audition();
        app.poll_capture_trim();
        app.poll_plugin_touch();
        app.poll_health();
        app.tick_automation();
    }

    /// Draw the whole interface through any ratatui backend — the plugin's
    /// window, or a `TestBackend` in a test. The same [`ui`] the terminal draws.
    pub fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        terminal.draw(|f| ui(f, &mut self.app))?;
        Ok(())
    }

    /// A key from the host's window, handled exactly as the terminal's would be.
    pub fn key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        crate::handle_key(&mut self.app, code);
    }

    /// A mouse event from the host's window, in cells. choz's panels are as
    /// much a mouse interface as a keyboard one — the faders, the rack buttons
    /// and every drawer row are clickable — so a window that only took keys
    /// would be half an interface.
    pub fn mouse(&mut self, event: ratatui::crossterm::event::MouseEvent) {
        crate::handle_mouse(&mut self.app, event);
    }

    /// One MIDI message from the host's note port.
    ///
    /// It arrives as [`InputSource::Jack`](choz_engine::input::InputSource),
    /// which is the source choz already means by "the sequence a DAW is
    /// playing" — so which tab listens to the host, and what its notes do when
    /// they get there, is the wiring the rack already has.
    pub fn midi(&mut self, bytes: &[u8]) {
        let source = choz_engine::input::InputSource::Jack;
        if let Some(event) = choz_engine::midi::event_of(bytes, source, &mut self.bank) {
            let _ = self.app.note_tx.send(event);
        }
    }

    /// The rack as a project file's YAML: what the host saves with its session.
    pub fn save_state(&mut self) -> Result<String> {
        Ok(serde_yaml::to_string(&self.app.project_snapshot())?)
    }

    /// Put a saved rack back. Rebuilds every tab, so it is slow and belongs
    /// nowhere near the audio thread.
    pub fn load_state(&mut self, yaml: &str) -> Result<()> {
        let project: crate::project::Project = serde_yaml::from_str(yaml)?;
        self.app.apply_project(project);
        Ok(())
    }

    /// Whether the rack is playing, for a host that wants to follow.
    pub fn tabs(&self) -> usize {
        self.app.slots.len()
    }
}
