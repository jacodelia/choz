//! Manual check: stream into an output at a given buffer size under a realistic
//! rack load, so xruns can be counted from outside (pw-top).
//! `cargo run -p choz-engine --example latency_probe -- <buffer> <seconds> [sink] [slots]`

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let buffer: u32 = args.next().unwrap_or_else(|| "256".into()).parse()?;
    let secs: u64 = args.next().unwrap_or_else(|| "10".into()).parse()?;
    let sink = args.next();
    let slots: usize = args.next().unwrap_or_else(|| "0".into()).parse()?;

    let mut engine = choz_engine::AudioEngine::new(48_000, buffer);
    if let Some(ref s) = sink {
        engine.set_output_device_preference(s);
    }
    engine.start()?;

    let sf2 = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
    for i in 0..slots {
        let Some(slot) = engine.add_silent() else {
            break;
        };
        engine.load_sf2(slot, sf2, 0, (i * 8) as u8)?;
    }
    engine.set_playing(true);

    // Hold a chord per slot: polyphony is what actually loads the RT thread.
    for i in 0..slots {
        for n in [48, 55, 60, 64, 67, 72] {
            engine.note_on(i, n, 100);
        }
    }
    println!(
        "buffer={buffer} slots={slots} out={:?}",
        engine.output_device()
    );

    // Starting up (and re-negotiating the quantum) always costs a few xruns;
    // only what happens after the graph settles says anything about latency.
    std::thread::sleep(std::time::Duration::from_secs(5));
    println!("MARK");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Ok(())
}
