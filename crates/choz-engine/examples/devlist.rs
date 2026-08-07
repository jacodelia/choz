//! Manual check: list the outputs the engine offers and hop between them.
//! `cargo run -p choz-engine --example devlist`

fn main() -> anyhow::Result<()> {
    let mut engine = choz_engine::AudioEngine::new(48_000, 256);
    engine.start()?;
    println!("backend: {}", engine.backend.label());
    println!("current: {:?}", engine.output_device());

    let devs = engine.output_devices();
    for d in &devs {
        println!("  - {d}");
    }
    for d in &devs {
        match engine.set_output_device(d) {
            Ok(rebuilt) => println!(
                "switch to {d}: ok (rebuilt: {rebuilt}) -> now {:?}",
                engine.output_device()
            ),
            Err(e) => println!("switch to {d}: {e}"),
        }
    }
    Ok(())
}
