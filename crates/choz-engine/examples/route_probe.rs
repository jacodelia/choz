//! Manual check: native JACK client with per-slot output routing. One SF2 note
//! per output pair, so which jack of the interface it comes out of is audible.
//! `cargo run -p choz-engine --example route_probe -- [sink] [secs]`

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let sink = args.next().filter(|s| s != "-");
    let secs: u64 = args.next().unwrap_or_else(|| "6".into()).parse()?;

    let mut engine = choz_engine::AudioEngine::new(48_000, 256);
    if let Some(ref s) = sink {
        engine.set_output_device_preference(s);
    }
    engine.start()?;
    println!(
        "backend={} out={}ch in={}ch dev={:?}",
        engine.backend.label(),
        engine.output_channels(),
        engine.input_channels(),
        engine.output_device()
    );

    let sf2 = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
    // One slot per output pair, each on its own note, so the routing is audible.
    for pair in 0..(engine.output_channels() / 2) {
        let Some(slot) = engine.add_silent() else {
            break;
        };
        engine.load_sf2(slot, sf2, 0, 0)?;
        engine.set_slot_out(slot, pair * 2, pair * 2 + 1);
        engine.note_on(slot, 48 + (pair as u8) * 4, 100);
        println!(
            "slot {slot}: note {} -> out {}/{}",
            48 + pair * 4,
            pair * 2 + 1,
            pair * 2 + 2
        );
    }
    engine.set_playing(true);
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Ok(())
}
