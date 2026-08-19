//! The "pedal down on one tab, solo on the other" dropout, measured.
//!
//! ```sh
//! cargo run --release --example pedal_bench -p choz-engine -- \
//!     ~/sf2/DSoundFontV4.sf2 "/usr/lib/vst3/Surge XT.vst3" [sandbox|inproc] [busy threads]
//! ```
//!
//! Two slots, one callback, the rig the bug was reported on: 96 kHz at 128
//! frames = a 1.33 ms budget for *both* sources together. The SF2 holds a
//! pedalful of notes while the plugin gets played over the top, and every
//! block is timed. What matters is the tail, not the mean: one block over
//! budget is an xrun, and an xrun is the sound stopping.

use choz_engine::sources::{AudioSource, Sf2Synth};
use std::time::Instant;

const SR: u32 = 96_000;
const BLOCK: usize = 128;

fn main() {
    // The sandbox child is this binary again, so it has to answer the worker
    // flag before anything else — exactly like the choz binary does.
    if choz_engine::worker_main() {
        return;
    }
    let mut args = std::env::args().skip(1);
    let sf2 = args
        .next()
        .expect("usage: pedal_bench <sf2> <vst3> [sandbox|inproc]");
    let vst3 = args
        .next()
        .expect("usage: pedal_bench <sf2> <vst3> [sandbox|inproc]");
    let sandbox = args.next().as_deref() != Some("inproc");
    // A busy machine, which is the state the bug is reported in: the UI
    // thread painting, a browser, whatever else. Normal priority, like every
    // one of those — and like the sandbox child.
    let load: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(0);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for _ in 0..load {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        });
    }
    // The audio thread is a realtime thread — that is the whole point of the
    // inversion this measures: an RT thread waiting on a normal-priority child.
    unsafe {
        let p = libc::sched_param { sched_priority: 80 };
        if libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &p) != 0 {
            eprintln!("(not realtime: run as a member of @audio for the real thing)");
        }
    }

    let mut piano = Sf2Synth::load(std::path::Path::new(&sf2), 0, 0, SR).expect("load SF2");
    let path = std::path::Path::new(&vst3);
    let mut lead: Box<dyn AudioSource> = if sandbox {
        Box::new(
            choz_engine::sandboxed::SandboxedPlugin::build(
                choz_engine::PluginFormat::Vst3,
                path,
                "",
                SR,
                BLOCK as u32,
            )
            .expect("sandbox Surge"),
        )
    } else {
        choz_engine::engine::build_instrument(
            choz_engine::PluginFormat::Vst3,
            path,
            "",
            SR,
            BLOCK as u32,
        )
        .expect("load Surge")
    };

    // Left hand under the pedal: 24 notes down, none of them released.
    piano.control_change(64, 127);
    for n in (36u8..=59).step_by(1) {
        piano.note_on(n, 100);
        piano.note_off(n);
    }

    let budget = BLOCK as f64 / SR as f64 * 1000.0;
    let mut a = vec![0.0f32; BLOCK * 2];
    let mut b = vec![0.0f32; BLOCK * 2];
    let mut times = Vec::with_capacity(4000);
    // Paced like the real callback: one block every period, with the thread
    // idle in between. Running flat out is the one thing the audio thread
    // never does, and it hides the child's wake-up latency entirely.
    let period = std::time::Duration::from_secs_f64(BLOCK as f64 / SR as f64);
    let start = Instant::now();
    // Right hand on the other tab: a note every 16 blocks (~21 ms), which is
    // a fast solo, not an unreasonable one.
    for i in 0..4000u32 {
        let i = i as usize;
        if i.is_multiple_of(16) {
            lead.note_off(60 + (i / 16 % 12) as u8);
            lead.note_on(60 + (i / 16 % 12) as u8, 100);
        }
        while start.elapsed() < period * i as u32 {
            std::hint::spin_loop();
        }
        let t = Instant::now();
        piano.render(&mut a, SR);
        lead.render(&mut b, SR);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let missed = sandbox_missed(lead.as_ref());
    times.sort_by(f64::total_cmp);
    let over = times.iter().filter(|t| **t > budget).count();
    let pct = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
    println!(
        "{:<8} budget {budget:.2} ms | median {:.3} | p99 {:.3} | max {:.3} | over budget {over}/{} blocks ({:.1}%)",
        if sandbox { "sandbox" } else { "inproc" },
        pct(0.5),
        pct(0.99),
        times[times.len() - 1],
        times.len(),
        over as f64 / times.len() as f64 * 100.0
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(m) = missed {
        println!("         child missed {m} blocks (silence for each one)");
    }
}

/// The miss count, when the source is a sandboxed one.
fn sandbox_missed(src: &dyn AudioSource) -> Option<u64> {
    src.sandbox()
        .map(|s| s.missed.load(std::sync::atomic::Ordering::Relaxed))
}
