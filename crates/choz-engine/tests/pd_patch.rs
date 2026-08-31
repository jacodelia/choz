//! A real Pure Data patch, running in a real child process.
//!
//! Skipped — loudly, not silently — when `choz-pd-host` has not been built:
//! that binary is the only thing that links libpd, and it is behind a feature
//! precisely so choz builds on a machine without Pure Data.
//!
//! ```bash
//! cargo build -p choz-plugin-pd --features pd
//! cargo test -p choz-engine --test pd_patch
//! ```

use std::path::PathBuf;

use choz_engine::fx_chain::{build_chain_from_specs, FxSpec, PluginFxRef};
use choz_engine::PluginFormat;

const SR: u32 = 48_000;
const FRAMES: u32 = 256;

/// The child binary, wherever this build put it: `CHOZ_PD_HOST` first, then
/// beside the test binary's own target directory.
fn pd_host() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CHOZ_PD_HOST") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    // …/target/debug/deps/pd_patch-<hash> → …/target/debug/choz-pd-host
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let candidate = dir.join("choz-pd-host");
    candidate.is_file().then_some(candidate)
}

fn main() {
    let Some(host) = pd_host() else {
        eprintln!(
            "choz-pd-host is not built; skipping. \
             Build it with `cargo build -p choz-plugin-pd --features pd`."
        );
        return;
    };
    // The engine looks for it next to the choz binary; here it is next to the
    // test binary, so point at it explicitly.
    unsafe { std::env::set_var("CHOZ_PD_HOST", &host) };

    let dir = std::env::temp_dir().join("choz-pd-patch-test");
    std::fs::create_dir_all(&dir).unwrap();
    let patch = dir.join("gain.pd");
    std::fs::write(
        &patch,
        "#N canvas 0 0 450 300 12;\n\
         #X obj 50 50 adc~;\n\
         #X obj 50 100 *~ 0.5;\n\
         #X obj 50 150 dac~;\n\
         #X connect 0 0 1 0;\n\
         #X connect 1 0 2 0;\n\
         #X connect 1 0 2 1;\n",
    )
    .unwrap();

    // Exactly the road a patch takes when the user picks it in ADD FX: an FX
    // spec with a plugin reference, built into a chain.
    let spec = FxSpec {
        gate: None,
        kind: String::new(),
        enabled: true,
        wet: 1.0,
        params: Vec::new(),
        plugin: Some(PluginFxRef {
            format: PluginFormat::Pd,
            path: patch.clone(),
            id: String::new(),
        }),
        loops: Vec::new(),
        loop_frames: 0,
    };
    let mut chain = build_chain_from_specs(&[spec], SR, FRAMES);
    assert_eq!(chain.len(), 1, "the patch should have become an effect");

    // The patch halves what it is given, so the answer says three things at
    // once: the child started, the block crossed both ways, and Pd's DSP is
    // actually on (with it off the output is exactly zero).
    let mut buf = vec![0.4f32; (FRAMES * 2) as usize];
    chain[0].process_block(&mut buf, SR);
    assert!(
        (buf[0] - 0.2).abs() < 1e-4,
        "the patch halves it: got {} (0.0 means Pd computed nothing)",
        buf[0]
    );

    // And it keeps up block after block, which is what a sandboxed effect has
    // to do to be usable at all.
    for i in 0..50 {
        buf.fill(0.4);
        chain[0].process_block(&mut buf, SR);
        assert!(
            (buf[0] - 0.2).abs() < 1e-4,
            "block {i} came back as {}",
            buf[0]
        );
    }

    // A patch driven by its own controls: choz moves the slider and the patch
    // answers. Silent with the slider down, loud with it up — which is the
    // whole reason a headless host has to be able to reach them.
    let knobs = dir.join("knob.pd");
    std::fs::write(
        &knobs,
        "#N canvas 0 0 450 300 12;\n\
         #X obj 20 20 adc~ 1;\n\
         #X obj 20 60 *~;\n\
         #X obj 20 100 dac~;\n\
         #X obj 200 40 hsl 170 20 0 1 0 0 empty gain GAIN -2 -10 0 12 #c6ffc7 #000000 #000000 0 1;\n\
         #X connect 0 0 1 0;\n\
         #X connect 1 0 2 0;\n\
         #X connect 1 0 2 1;\n\
         #X connect 3 0 1 1;\n",
    )
    .unwrap();
    let params = choz_engine::read_plugin_params(PluginFormat::Pd, &knobs, "");
    assert_eq!(params.len(), 1, "the named slider is a knob: {params:?}");
    assert_eq!(params[0].name, "GAIN");

    let spec = FxSpec {
        gate: None,
        kind: String::new(),
        enabled: true,
        wet: 1.0,
        params: Vec::new(),
        plugin: Some(PluginFxRef {
            format: PluginFormat::Pd,
            path: knobs,
            id: String::new(),
        }),
        loops: Vec::new(),
        loop_frames: 0,
    };
    let mut chain = build_chain_from_specs(&[spec], SR, FRAMES);
    assert_eq!(chain.len(), 1);
    let peak_with = |chain: &mut Vec<Box<dyn choz_ports::FxProcessor>>, gain: f32| -> f32 {
        chain[0].set_param(0, gain);
        let mut peak = 0.0f32;
        for block in 0..40 {
            let mut buf = vec![0.4f32; (FRAMES * 2) as usize];
            let _ = block;
            chain[0].process_block(&mut buf, SR);
            peak = peak.max(buf.iter().fold(0.0f32, |a, s| a.max(s.abs())));
        }
        peak
    };
    assert!(peak_with(&mut chain, 0.0) < 0.01, "slider down is silence");
    assert!(
        peak_with(&mut chain, 1.0) > 0.2,
        "slider up is the signal — and note the `hsl` line above is written in \
         full: Pd will not create one with fields missing, while choz's own \
         reader is happy with either"
    );
    drop(chain);

    // The same patch with the slider **unnamed** — `empty empty`, which is how
    // Pd saves one unless somebody typed a receive symbol into its properties,
    // and therefore how nearly every patch in the wild is. choz names it in the
    // copy it plays, so it works exactly like the one above.
    let unnamed = dir.join("unnamed.pd");
    std::fs::write(
        &unnamed,
        "#N canvas 0 0 450 300 12;\n\
         #X obj 20 20 adc~ 1;\n\
         #X obj 20 60 *~;\n\
         #X obj 20 100 dac~;\n\
         #X obj 200 40 hsl 170 20 0 1 0 0 empty empty Gain -2 -10 0 12 #c6ffc7 #000000 #000000 0 1;\n\
         #X connect 0 0 1 0;\n\
         #X connect 1 0 2 0;\n\
         #X connect 1 0 2 1;\n\
         #X connect 3 0 1 1;\n",
    )
    .unwrap();
    let params = choz_engine::read_plugin_params(PluginFormat::Pd, &unnamed, "");
    assert_eq!(params.len(), 1, "unnamed sliders are knobs too: {params:?}");
    assert_eq!(params[0].name, "Gain", "and keep the patch's own label");

    let spec = FxSpec {
        gate: None,
        kind: String::new(),
        enabled: true,
        wet: 1.0,
        params: Vec::new(),
        plugin: Some(PluginFxRef {
            format: PluginFormat::Pd,
            path: unnamed,
            id: String::new(),
        }),
        loops: Vec::new(),
        loop_frames: 0,
    };
    let mut chain = build_chain_from_specs(&[spec], SR, FRAMES);
    assert_eq!(chain.len(), 1);
    assert!(peak_with(&mut chain, 0.0) < 0.01, "slider down is silence");
    assert!(
        peak_with(&mut chain, 1.0) > 0.2,
        "slider up is the signal — the patch itself names nothing, so this is \
         the copy choz filled in doing its job"
    );
    drop(chain);

    // A patch with nothing to connect audio to is not offered as an effect —
    // the scan reads the file and says so, without Pure Data being involved.
    std::fs::write(
        dir.join("gui.pd"),
        "#N canvas 0 0 450 300 12;\n#X obj 20 20 bng 15 250 50 0;\n",
    )
    .unwrap();
    let found = choz_engine::paths::scan_dir(&dir, PluginFormat::Pd);
    let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"gain") && names.contains(&"knob"),
        "the effects are listed: {names:?}"
    );
    assert!(
        !names.contains(&"gui"),
        "and the one that connects to nothing is not: {names:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    println!("pd_patch: ok");
}
