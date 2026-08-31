//! A real plugin, playing in a real child process.
//!
//! Custom harness (`harness = false`): the test binary is the sandbox worker
//! too, the same trick the scan and probe tests use.

use choz_engine::sandboxed::SandboxedPlugin;
use choz_engine::PluginFormat;
use choz_ports::AudioSource;

const SR: u32 = 48_000;
const FRAMES: u32 = 256;

fn main() {
    // Any of the three worker roles: the engine re-runs this binary for all
    // of them, and answering only one means the others re-enter the test.
    if choz_engine::worker_main() {
        return;
    }

    // An effect is the easiest thing to check: feed it a signal and see it come
    // back. Zam's VST2 build is on every machine that has the Zam plugins.
    let fx = std::path::Path::new("/usr/lib/vst/ZamComp-vst.so");
    if !fx.exists() {
        eprintln!("no VST2 effect installed; skipping");
        return;
    }

    let mut plug = SandboxedPlugin::build(PluginFormat::Vst2, fx, "", SR, FRAMES)
        .expect("the plugin should start in its sandbox");

    // The host end is an ordinary AudioSource: a rack slot can't tell.
    let mut out = vec![0.0f32; (FRAMES * 2) as usize];
    for i in 0..50 {
        let frames = plug.render(&mut out, SR);
        assert_eq!(frames, FRAMES as usize, "block {i} came back short");
        assert!(out.iter().all(|s| s.is_finite()), "block {i} is not finite");
    }
    assert_eq!(plug.missed(), 0, "the child kept up with every block");

    // The plugin's window is opened **by the child**, in the child's process:
    // that is what stops a crashing GUI from taking choz down. No X11 display
    // is needed here — what is checked is that the request crosses, the child
    // acts on it and answers, and that audio keeps flowing meanwhile.
    {
        let editor = choz_ports::AudioSource::editor(&plug).expect("a sandboxed plugin offers one");
        // A window id no server knows: the plugin refuses to embed and the
        // child says so, which is still a complete round trip.
        let _ = editor.open(0x1);
        editor.close();

        let before = plug.missed();
        for _ in 0..20 {
            plug.render(&mut out, SR);
        }
        assert_eq!(
            plug.missed(),
            before,
            "the window traffic did not disturb the audio"
        );
    }

    // A block smaller than the region still works — the callback decides the
    // size, not us.
    let mut short = vec![0.0f32; 64];
    assert_eq!(plug.render(&mut short, SR), 32);

    // …and a bigger one is chunked.
    let mut long = vec![0.0f32; (FRAMES * 2) as usize * 3];
    assert_eq!(plug.render(&mut long, SR), FRAMES as usize * 3);
    assert!(long.iter().all(|s| s.is_finite()));

    drop(plug);
    println!("test a_plugin_plays_through_the_sandbox ... ok");

    a_plugin_that_cannot_be_destroyed_is_sandboxed_automatically();
    a_killed_child_comes_back_by_itself();
    an_effect_processes_through_the_sandbox();
    a_plugin_the_user_asked_for_runs_out_of_process();
    only_the_sandbox_offers_an_editor_choz_itself_refuses();
}

/// guitarix's X11 UIs segfault whatever loads them, so choz's own process is
/// never offered one. A sandbox child is a process choz can afford to lose —
/// there the editor comes back, and the host learns about it from the child
/// rather than from its own (refusing) discovery.
fn only_the_sandbox_offers_an_editor_choz_itself_refuses() {
    let bundle = std::path::Path::new("/usr/lib/lv2/gxts9.lv2");
    let uri = "http://guitarix.sourceforge.net/plugins/gxts9#ts9sim";
    if !bundle.exists() {
        eprintln!("guitarix not installed; skipping");
        return;
    }

    // In here — choz's process — the bundle's editor is hidden.
    let found = choz_plugin_lv2::discovery::discover_bundle(bundle);
    let info = found
        .iter()
        .find(|p| p.uri == uri)
        .expect("gxts9 is in its bundle");
    assert!(
        info.ui.is_none(),
        "choz's own process must not be offered this UI"
    );

    // …which is why the sandbox is where this UI can be opened at all: the
    // probe child looks past the deny-list (asking is not opening) and sees a
    // window. Whether that is *reason enough* to pay for a process is the
    // user's call now — `CHOZ_SANDBOX_GUI=1` — because the round trip costs
    // most of an audio block and a rack that runs out of time is a certain
    // failure, where a GUI crash is a possible one.
    let state = std::env::temp_dir().join(format!("choz_gui_sbx_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    unsafe { std::env::set_var("XDG_STATE_HOME", &state) };
    unsafe { std::env::set_var("CHOZ_SANDBOX_GUI", "1") };
    assert!(
        choz_engine::quarantine::wants_sandbox(PluginFormat::Lv2, bundle, uri),
        "asked for, a plugin with a window goes to its own process"
    );
    unsafe { std::env::remove_var("CHOZ_SANDBOX_GUI") };
    assert!(
        !choz_engine::quarantine::wants_sandbox(PluginFormat::Lv2, bundle, uri),
        "and by default it does not — the window alone is not worth the block"
    );
    let _ = std::fs::remove_dir_all(&state);

    let plug = SandboxedPlugin::build(PluginFormat::Lv2, bundle, uri, SR, FRAMES)
        .expect("gxts9 should load in its sandbox");
    assert!(
        choz_ports::AudioSource::editor(&plug).is_some(),
        "the child can afford the UI, so the GUI button comes back"
    );
    drop(plug);

    // The other half: a sandboxed plugin that has no window must not get a
    // `GUI` button either, or pressing it opens an empty frame. Ardour's
    // a-delay ships no X11 UI.
    let plain = std::path::Path::new("/usr/lib/lv2/a-delay.lv2");
    if plain.exists() {
        let plug =
            SandboxedPlugin::build(PluginFormat::Lv2, plain, "urn:ardour:a-delay", SR, FRAMES)
                .expect("a-delay should load in its sandbox");
        assert!(
            choz_ports::AudioSource::editor(&plug).is_none(),
            "no window in the child means no button in the host"
        );
    }
    println!("test only_the_sandbox_offers_an_editor_choz_itself_refuses ... ok");
}

/// The manual half of the policy: a plugin the probe found perfectly healthy
/// still goes into its own process once the user asks for it, and stops when
/// the user changes their mind.
fn a_plugin_the_user_asked_for_runs_out_of_process() {
    let path = std::path::Path::new("/usr/lib/vst/ZamComp-vst.so");
    if !path.exists() {
        return;
    }
    let state = std::env::temp_dir().join(format!("choz_forced_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    unsafe { std::env::set_var("XDG_STATE_HOME", &state) };

    // ZamComp is an effect, so the FX chain is where the policy applies to it —
    // the same `wants_sandbox` call the instrument path makes.
    let spec = || choz_engine::FxSpec {
        gate: None,
        kind: String::new(),
        enabled: true,
        wet: 1.0,
        params: Vec::new(),
        plugin: Some(choz_engine::fx_chain::PluginFxRef {
            format: PluginFormat::Vst2,
            path: path.to_path_buf(),
            id: String::new(),
        }),
        loops: Vec::new(),
        loop_frames: 0,
    };
    let build = || choz_engine::fx_chain::build_chain_from_specs(&[spec()], SR, FRAMES);

    // A window is **not** reason enough any more — the round trip costs most of
    // an audio block, which is a certain failure traded against a possible one.
    // It is still available for whoever wants it.
    unsafe { std::env::set_var("CHOZ_SANDBOX_GUI", "1") };
    assert!(
        choz_engine::quarantine::wants_sandbox(PluginFormat::Vst2, path, ""),
        "asked for, a plugin with a GUI is isolated on its own"
    );
    unsafe { std::env::remove_var("CHOZ_SANDBOX_GUI") };

    // Nothing wrong with it, so nothing sandboxes it on its own.
    assert!(!choz_engine::quarantine::wants_sandbox(
        PluginFormat::Vst2,
        path,
        ""
    ));
    let chain = build();
    assert!(
        chain[0].sandbox().is_none(),
        "a healthy plugin stays in-process"
    );
    drop(chain);

    choz_engine::quarantine::set_forced(PluginFormat::Vst2, path, "", true);
    assert!(choz_engine::quarantine::wants_sandbox(
        PluginFormat::Vst2,
        path,
        ""
    ));
    let mut chain = build();
    let status = chain[0]
        .sandbox()
        .expect("the user asked for its own process");

    let mut buf: Vec<f32> = (0..FRAMES * 2)
        .map(|i| (i as f32 * 0.05).sin() * 0.5)
        .collect();
    for _ in 0..20 {
        chain[0].process_block(&mut buf, SR);
    }
    assert!(buf.iter().all(|s| s.is_finite()));
    // The counters the RACK shows are live, not read off the instance.
    assert_eq!(status.missed(), 0, "the child kept up");
    assert_eq!(status.restarts(), 0, "nothing crashed");
    drop(chain);

    choz_engine::quarantine::set_forced(PluginFormat::Vst2, path, "", false);
    assert!(!choz_engine::quarantine::forced(
        PluginFormat::Vst2,
        path,
        ""
    ));
    let _ = std::fs::remove_dir_all(&state);
    println!("test a_plugin_the_user_asked_for_runs_out_of_process ... ok");
}

/// The FX-chain end of the same machinery: the dry signal goes across and
/// comes back processed, and choz's own wet/dry is applied on this side.
fn an_effect_processes_through_the_sandbox() {
    use choz_ports::FxProcessor;

    let path = std::path::Path::new("/usr/lib/vst/ZamComp-vst.so");
    if !path.exists() {
        return;
    }
    let mut fx =
        choz_engine::sandboxed::SandboxedEffect::build(PluginFormat::Vst2, path, "", SR, FRAMES)
            .expect("effect should start in its sandbox");

    // A loud sine in, something finite out — and not silence, which is what a
    // missed block would leave behind.
    let mut buf: Vec<f32> = (0..FRAMES * 2)
        .map(|i| (i as f32 * 0.05).sin() * 0.8)
        .collect();
    let before = buf.clone();
    fx.process_block(&mut buf, SR);
    assert!(buf.iter().all(|s| s.is_finite()));
    assert!(
        buf.iter().any(|s| *s != 0.0),
        "the effect answered with silence"
    );

    // Fully dry means the input comes back untouched, whatever the plugin did.
    let mut dry = before.clone();
    fx.set_mix(0.0);
    fx.process_block(&mut dry, SR);
    for (a, b) in dry.iter().zip(&before) {
        assert!((a - b).abs() < 1e-6, "dry should pass through: {a} vs {b}");
    }

    // A parameter change crosses without upsetting anything.
    fx.set_param(0, 0.75);
    fx.set_mix(1.0);
    fx.process_block(&mut buf, SR);
    assert!(buf.iter().all(|s| s.is_finite()));
    println!("test an_effect_processes_through_the_sandbox ... ok");
}

/// The other half of sandboxing: a plugin that dies doesn't just fail to take
/// choz down, it *comes back*. The tab is silent for a moment, not until the
/// user reloads it.
fn a_killed_child_comes_back_by_itself() {
    let fx = std::path::Path::new("/usr/lib/vst/ZamComp-vst.so");
    if !fx.exists() {
        return;
    }
    let mut plug = SandboxedPlugin::build(PluginFormat::Vst2, fx, "", SR, FRAMES)
        .expect("plugin should start");
    let mut out = vec![0.0f32; (FRAMES * 2) as usize];
    plug.render(&mut out, SR);

    let first = plug.child_pid();
    assert!(first > 0);
    // SAFETY: our own child, and SIGKILL is what a crash looks like from here.
    unsafe { libc::kill(first as i32, libc::SIGKILL) };

    // Keep the audio thread doing what it always does. The blocks in between
    // come back silent, and then a new child answers.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut pid = first;
    while std::time::Instant::now() < deadline {
        plug.render(&mut out, SR);
        pid = plug.child_pid();
        if pid != first && plug.restarts() > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_ne!(pid, first, "the sandbox should have started a new child");
    assert_eq!(plug.restarts(), 1);

    // And it is really playing again, not just alive.
    let missed_before = plug.missed();
    for _ in 0..20 {
        plug.render(&mut out, SR);
    }
    assert_eq!(
        plug.missed(),
        missed_before,
        "the new child answers every block"
    );
    println!("test a_killed_sandbox_child_is_replaced ... ok");
}

/// The policy: a plugin the load probe caught dying on teardown is hosted in
/// its own process, so destroying it costs a child, not the app.
///
/// padthv1 is that plugin here — in-process, this very function would segfault
/// at the `drop`.
fn a_plugin_that_cannot_be_destroyed_is_sandboxed_automatically() {
    let bundle = std::path::Path::new("/usr/lib/lv2/padthv1.lv2");
    if !bundle.exists() {
        eprintln!("padthv1 not installed; skipping the teardown-crash policy check");
        return;
    }
    // Keep the verdict cache out of the real state dir.
    let state = std::env::temp_dir().join(format!("choz_sbx_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    unsafe { std::env::set_var("XDG_STATE_HOME", &state) };

    let uri = "http://padthv1.sourceforge.net/lv2";
    assert_eq!(
        choz_engine::quarantine::check(PluginFormat::Lv2, bundle, uri).verdict,
        choz_engine::quarantine::Verdict::CrashesOnTeardown,
        "the probe should have caught it"
    );

    let mut plug =
        choz_engine::engine::build_hosted_instrument(PluginFormat::Lv2, bundle, uri, SR, FRAMES)
            .expect("padthv1 should load, sandboxed");

    let mut out = vec![0.0f32; (FRAMES * 2) as usize];
    plug.note_on(60, 100);
    for _ in 0..20 {
        plug.render(&mut out, SR);
    }
    assert!(out.iter().all(|s| s.is_finite()));

    // The moment of truth: in-process this drop takes the whole process with
    // it. Sandboxed, it is one child exiting.
    drop(plug);
    let _ = std::fs::remove_dir_all(&state);
    println!("test a_teardown_crasher_is_hosted_out_of_process ... ok");
}
