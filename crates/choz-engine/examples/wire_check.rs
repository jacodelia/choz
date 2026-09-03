//! The capture wiring, against the real graph.
//!
//! Starts the engine on the native JACK client, asks for one pair of capture
//! jacks and then for none, and counts what is actually connected each time.
//!
//! `cargo run --release -p choz-engine --example wire_check`

fn connected_to_us() -> usize {
    let Ok(out) = std::process::Command::new("jack_lsp").arg("-c").output() else {
        return 0;
    };
    // `jack_lsp -c` prints a port and its connections indented under it.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with(' ') || l.starts_with('\t'))
        .filter(|l| l.contains("choz:in_"))
        .count()
}

/// utime + stime of the audio server, in seconds.
fn server_cpu() -> f64 {
    let Ok(out) = std::process::Command::new("pgrep")
        .arg("-x")
        .arg("pipewire")
        .output()
    else {
        return 0.0;
    };
    let Some(pid) = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_string)
    else {
        return 0.0;
    };
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())) else {
        return 0.0;
    };
    let Some(rest) = stat.rsplit_once(") ") else {
        return 0.0;
    };
    let f: Vec<&str> = rest.1.split_whitespace().collect();
    let ticks: f64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0)
        + f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    ticks / 100.0
}

/// What the graph costs over `secs`, as a share of one core.
fn graph_cost(secs: u64) -> f64 {
    let before = server_cpu();
    let t = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_secs(secs));
    (server_cpu() - before) / t.elapsed().as_secs_f64() * 100.0
}

fn main() -> anyhow::Result<()> {
    let mut engine = choz_engine::AudioEngine::new(48_000, 256);
    engine.start()?;
    println!(
        "backend={} in={} ports",
        engine.backend.label(),
        engine.input_channels()
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("as started:        {} connections", connected_to_us());

    // What a rack listening to jacks 3 and 4 needs.
    engine.set_capture_wiring((1 << 2) | (1 << 3));
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("one stereo pair:   {} connections", connected_to_us());

    // Nobody listening.
    engine.set_capture_wiring(0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("nothing listening: {} connections", connected_to_us());

    // And the drawer open again.
    engine.set_capture_wiring(u64::MAX);
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("drawer open:       {} connections", connected_to_us());

    // What each of the two costs the graph, over the same window.
    engine.set_capture_wiring(u64::MAX);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let all = graph_cost(8);
    engine.set_capture_wiring((1 << 2) | (1 << 3));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let pair = graph_cost(8);
    println!("\neverything wired: {all:.2} % of a core");
    println!(
        "one pair wired:   {pair:.2} % of a core   ({:+.2})",
        pair - all
    );
    Ok(())
}
