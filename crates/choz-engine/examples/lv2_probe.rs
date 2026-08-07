//! Manual check: real stream + a real LV2 instrument slot, so xruns can be
//! counted from outside (pw-top). Mirrors `latency_probe` but with a plugin.
//! `cargo run -p choz-engine --example lv2_probe -- <uri-fragment> <buffer> <seconds>`

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let frag = args.next().unwrap_or_else(|| "JX10".into());
    let buffer: u32 = args.next().unwrap_or_else(|| "256".into()).parse()?;
    let secs: u64 = args.next().unwrap_or_else(|| "10".into()).parse()?;

    let found = choz_plugin_lv2::scan_directory(std::path::Path::new("/usr/lib/lv2"));
    let Some(info) = found.iter().find(|p| p.is_instrument && p.uri.contains(&frag)) else {
        println!("no LV2 instrument matching {frag}");
        return Ok(());
    };

    let mut engine = choz_engine::AudioEngine::new(48_000, buffer);
    engine.start()?;
    let Some(slot) = engine.add_silent() else { return Ok(()) };
    engine.load_plugin(
        slot,
        choz_engine::PluginFormat::Lv2,
        &info.bundle_dir,
        &info.uri,
    )?;
    engine.set_playing(true);
    for n in [48, 55, 60, 64, 67, 72] {
        engine.note_on(slot, n, 100);
    }
    println!("{} buffer={buffer} out={:?}", info.uri, engine.output_device());
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Ok(())
}
