//! `choz Mic` against the real graph: what a meeting app (Zoom, Meet) records
//! while choz plays out of a given interface.
//!
//! Starts the engine on the native JACK client wired to `sink`, plays one SF2
//! note, and records `choz_mic` through the PulseAudio layer at 48 kHz — the
//! way a browser or Zoom opens it. Checks that the main pair reaches both the
//! interface and the microphone, that the microphone's own ports are not
//! offered as a choz input (a feedback loop), and that the take is not silent
//! or clipped, in stereo and summed to mono.
//!
//! `cargo run -p choz-engine --example mic_probe -- [sink]` (no sink: list them)

use std::io::Read;
use std::process::{Command, Stdio};

fn main() -> anyhow::Result<()> {
    let sink = std::env::args().nth(1);
    let mut engine = choz_engine::AudioEngine::new(48_000, 256);
    if let Some(ref s) = sink {
        engine.set_output_device_preference(s);
    }
    engine.start()?;
    println!(
        "backend={} rate={} out={}ch dev={:?}",
        engine.backend.label(),
        engine.sample_rate,
        engine.output_channels(),
        engine.output_device()
    );
    if sink.is_none() {
        println!("sinks: {:#?}", engine.output_devices());
        return Ok(());
    }
    let mut failed = Vec::new();
    if engine
        .input_ports()
        .iter()
        .any(|p| p.starts_with("choz Mic:"))
    {
        failed.push("choz Mic is offered as an input");
    }

    let slot = engine.add_silent().expect("a slot");
    engine.load_sf2(
        slot,
        std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2"),
        0,
        0,
    )?;
    engine.set_slot_out(slot, 0, 1);
    engine.set_playing(true);
    std::thread::sleep(std::time::Duration::from_millis(500));

    let wiring =
        String::from_utf8_lossy(&Command::new("jack_lsp").arg("-c").output()?.stdout).into_owned();
    let under = |port: &str| -> Vec<String> {
        wiring
            .lines()
            .skip_while(|l| *l != port)
            .skip(1)
            .take_while(|l| l.starts_with(' '))
            .map(|l| l.trim().to_string())
            .collect()
    };
    let out1 = under("choz:out_1");
    println!("choz:out_1 -> {out1:?}");
    if !out1.iter().any(|p| p == "choz Mic:input_FL") {
        failed.push("out_1 does not reach choz Mic");
    }
    if !out1.iter().any(|p| !p.starts_with("choz Mic:")) {
        failed.push("out_1 does not reach the interface");
    }

    let mut rec = Command::new("parec")
        .args([
            "-d",
            "choz_mic",
            "--rate=48000",
            "--channels=2",
            "--format=float32le",
        ])
        .args(["--latency-msec=20"])
        .stdout(Stdio::piped())
        .spawn()?;
    // Drained while it records: a pipe left full blocks parec at 64 KiB.
    let mut out = rec.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = out.read_to_end(&mut raw);
        raw
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    engine.note_on(slot, 60, 110);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    engine.note_off(slot, 60);
    rec.kill()?;
    let raw = reader.join().unwrap();
    let s: Vec<f32> = raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let peak = |it: &mut dyn Iterator<Item = f32>| it.fold(0f32, |m, x| m.max(x.abs()));
    let db = |p: f32| 20.0 * p.max(1e-9).log10();
    let (l, r) = (
        peak(&mut s.iter().step_by(2).copied()),
        peak(&mut s.iter().skip(1).step_by(2).copied()),
    );
    let mono = peak(&mut s.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5));
    println!(
        "choz Mic @48k: {} frames, L {:.1} dBFS, R {:.1} dBFS, mono {:.1} dBFS",
        s.len() / 2,
        db(l),
        db(r),
        db(mono)
    );
    if l.max(r) < 1e-3 {
        failed.push("choz Mic is silent");
    }
    if l.max(r) >= 1.0 {
        failed.push("choz Mic clips");
    }
    // A stereo patch summed to mono losing more than 6 dB is phase trouble.
    if db(mono) < db(l.max(r)) - 6.0 {
        failed.push("mono sum loses > 6 dB");
    }
    drop(engine);
    match failed.is_empty() {
        true => println!("OK"),
        false => anyhow::bail!("FAILED: {}", failed.join("; ")),
    }
    Ok(())
}
