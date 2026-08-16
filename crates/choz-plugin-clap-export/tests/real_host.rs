//! The exported bundle, loaded the way a DAW loads it.
//!
//! The unit tests next to the code call the ABI in this process, which proves
//! the vtables answer. This one goes the whole way: the built `.so` is copied
//! to a `.clap`, opened with **choz's own CLAP host** (`clack-host`, dlopen,
//! `clap_entry` looked up by symbol, the factory queried, an instance
//! activated) and given a block of audio. If this passes, Bitwig and Carla are
//! doing the same thing with the same file.

use std::path::PathBuf;

use choz_ports::FxProcessor;

/// The cdylib cargo just built for this crate.
fn bundle() -> Option<PathBuf> {
    // …/target/debug/deps/real_host-<hash> → …/target/debug/lib….so
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let so = dir.join("libchoz_plugin_clap_export.so");
    so.is_file().then_some(so)
}

#[test]
fn a_clap_host_loads_the_bundle_and_hears_an_effect() {
    let Some(so) = bundle() else {
        // Never silent: a skipped test that says nothing is a test that rots.
        panic!("the cdylib was not built — `cargo build -p choz-plugin-clap-export` first");
    };
    let dir = std::env::temp_dir().join("choz-clap-export-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A `.clap` *is* a shared object with a different extension. That is the
    // whole packaging story on Linux.
    let bundle = dir.join("choz.clap");
    std::fs::copy(&so, &bundle).unwrap();

    // Exactly what choz does when it scans a plugin directory.
    let found = choz_plugin_clap::scan_directory(&dir);
    assert_eq!(
        found.len(),
        choz_engine::fx_chain::BUILT_IN_KINDS.len(),
        "one file, every effect: {found:?}"
    );
    let gain = found
        .iter()
        .find(|p| p.id == "org.choz.fx.gain")
        .expect("the catalogue is what the host sees");
    assert_eq!(gain.name, "Gain");

    // And instantiated through the host, not through this crate's own statics.
    let mut effect = choz_plugin_clap::host::ClapEffect::build(&bundle, &gain.id, 48_000, 256)
        .expect("the host instantiated the plugin");

    // Gain's knob to the top, through the host's parameter path, and the block
    // has to come back louder — proof that the value crossed the boundary and
    // reached choz's DSP on the other side.
    effect.set_param(0, 1.0);
    let mut buf = vec![0.25f32; 256 * 2];
    effect.process_block(&mut buf, 48_000);
    assert!(
        buf[0] > 0.25 && buf.iter().all(|s| s.is_finite()),
        "the effect processed the block: {}",
        buf[0]
    );

    // The parameters a host would draw are the effect's own names, not indices.
    let params = choz_plugin_clap::read_params(&bundle, &gain.id);
    assert!(!params.is_empty(), "the plugin publishes its knobs");
    assert!(
        params.iter().any(|p| p.name == "Mix"),
        "the dry/wet travels with it: {:?}",
        params.iter().map(|p| &p.name).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
