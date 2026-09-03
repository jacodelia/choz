//! What a registered JACK port costs, and what a **connected** one costs.
//!
//! The roadmap has a point about registering only the ports a rack uses: on a
//! UMC1820 choz registers 34 of them and the graph moves all 34 every block,
//! used or not — measured at 5.5 % of a core with choz stopped. What that
//! measurement did not separate is *which* half costs: a port that exists, or a
//! port with something wired to it. The answer decides whether the point can be
//! done at all, because the level each row of the IN drawer shows comes from
//! our own ports — unregister them and the rows go blind.
//!
//! Three cases, same client, same seconds each: no ports, N registered and
//! silent, N registered and wired to whatever capture the graph has. What is
//! measured is the **server's** CPU time (`/proc/<pid>/stat`), which is where
//! the graph thread lives.
//!
//! `cargo run --release -p choz-engine --example port_cost -- [ports] [secs]`

use std::time::{Duration, Instant};

/// utime + stime of a process, in seconds.
fn cpu_of(pid: u32) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command can contain spaces and brackets; everything after `) ` is
    // fixed-width, and utime/stime are fields 14 and 15 from there.
    let rest = stat.rsplit_once(") ")?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

fn server_pid() -> Option<u32> {
    for name in ["pipewire", "jackd", "jackdbus"] {
        let out = std::process::Command::new("pgrep")
            .arg("-x")
            .arg(name)
            .output()
            .ok()?;
        if let Some(first) = String::from_utf8_lossy(&out.stdout).lines().next() {
            if let Ok(pid) = first.trim().parse() {
                return Some(pid);
            }
        }
    }
    None
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let ports: usize = args.next().unwrap_or_else(|| "34".into()).parse()?;
    let secs: u64 = args.next().unwrap_or_else(|| "6".into()).parse()?;

    let Some(pid) = server_pid() else {
        eprintln!("no PipeWire or JACK server running; nothing to measure");
        return Ok(());
    };
    println!("server pid {pid}, {ports} ports, {secs} s per case\n");

    let measure = |label: &str, open: usize, wire: bool| -> anyhow::Result<f64> {
        let client = match open {
            0 => None,
            n => {
                let (c, _) =
                    jack::Client::new("choz-port-cost", jack::ClientOptions::NO_START_SERVER)?;
                let ins: Vec<jack::Port<jack::AudioIn>> = (0..n)
                    .map(|i| c.register_port(&format!("in_{}", i + 1), jack::AudioIn))
                    .collect::<Result<_, _>>()?;
                let names: Vec<String> = ins.iter().filter_map(|p| p.name().ok()).collect();
                // A client that does nothing per block: what is being measured
                // is the graph moving buffers, not our DSP.
                let active = c.activate_async(
                    (),
                    jack::ClosureProcessHandler::new(
                        move |_: &jack::Client, _: &jack::ProcessScope| jack::Control::Continue,
                    ),
                )?;
                if wire {
                    let sources = active.as_client().ports(
                        None,
                        Some("32 bit float mono audio"),
                        jack::PortFlags::IS_OUTPUT,
                    );
                    let mut wired = 0;
                    for (from, ours) in sources
                        .iter()
                        .filter(|p| p.contains("capture"))
                        .zip(names.iter())
                    {
                        if active.as_client().connect_ports_by_name(from, ours).is_ok() {
                            wired += 1;
                        }
                    }
                    println!("  ({wired} of {n} wired — the graph has that many capture ports)");
                }
                Some(active)
            }
        };
        // Let the graph settle before the clock starts.
        std::thread::sleep(Duration::from_millis(500));
        let before = cpu_of(pid).unwrap_or(0.0);
        let t = Instant::now();
        std::thread::sleep(Duration::from_secs(secs));
        let used = cpu_of(pid).unwrap_or(0.0) - before;
        let wall = t.elapsed().as_secs_f64();
        drop(client);
        std::thread::sleep(Duration::from_millis(500));
        let pct = used / wall * 100.0;
        println!("{label:<28} {pct:>5.2} % of a core");
        Ok(pct)
    };

    let idle = measure("nothing of ours", 0, false)?;
    let registered = measure("registered, wired to nothing", ports, false)?;
    let connected = measure("registered and connected", ports, true)?;

    println!(
        "\nregistering {ports} ports costs {:+.2} points; wiring them costs {:+.2} more",
        registered - idle,
        connected - registered
    );
    Ok(())
}
