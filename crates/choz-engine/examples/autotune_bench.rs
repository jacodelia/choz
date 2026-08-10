//! What AutoTune costs, measured rather than asserted.
//!
//! ```sh
//! cargo run --release --example autotune_bench -p choz-engine
//! ```
//!
//! Two questions, and both have to be answered by running the thing:
//!
//! 1. **Does it allocate on the audio thread?** A global allocator that counts
//!    is the only answer that is not a promise. The count is taken around
//!    `process_block` alone, after everything is warm.
//! 2. **Does it fit in the buffer?** A 64-frame buffer at 48 kHz is 1.33 ms of
//!    wall clock; anything approaching that is a stream that will break.
//!
//! An example rather than a `#[bench]` because choz has no benchmark harness
//! and this needs none: it is a program that prints numbers, and it lives next
//! to `route_probe` and `latency_probe`, which are the same idea.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use choz_engine::fx::AutoTune;
use choz_ports::FxProcessor;

/// Counts every allocation the process makes. A global allocator is fine here
/// precisely because this is its own binary.
struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn voice(phase: &mut f32, hz: f32, sr: f32, frames: usize) -> Vec<f32> {
    let step = std::f32::consts::TAU * hz / sr;
    (0..frames)
        .flat_map(|_| {
            let p = *phase;
            let s = 0.2
                * (0.5 * p.sin()
                    + (2.0 * p).sin()
                    + 0.6 * (3.0 * p).sin()
                    + 0.25 * (4.0 * p).sin());
            *phase = (*phase + step) % std::f32::consts::TAU;
            [s, s]
        })
        .collect()
}

fn main() {
    println!("AutoTune — cost per buffer\n");
    for &sr in &[44_100.0f32, 48_000.0, 96_000.0] {
        for &frames in &[64usize, 128, 256, 512] {
            let mut at = AutoTune::new(sr);
            let mut phase = 0.0f32;

            // Warm up: fill the detector's window and the shifter's latency, and
            // let the allocator settle. Only what happens after this counts.
            let mut blocks: Vec<Vec<f32>> = (0..64)
                .map(|_| voice(&mut phase, 233.0, sr, frames))
                .collect();
            for b in blocks.iter_mut() {
                at.process_block(b, sr as u32);
            }

            let runs = 2000;
            let n = blocks.len();
            let before = ALLOCS.load(Ordering::Relaxed);
            let start = Instant::now();
            for i in 0..runs {
                at.process_block(&mut blocks[i % n], sr as u32);
            }
            let elapsed = start.elapsed();
            let allocs = ALLOCS.load(Ordering::Relaxed) - before;

            let per_buffer = elapsed.as_secs_f64() / runs as f64;
            let budget = frames as f64 / sr as f64;
            println!(
                "{:>6.0} Hz  {frames:>4} frames   {:>7.1} µs/buffer   {:>5.2} % of the \
                 {:.2} ms budget   allocations: {allocs}",
                sr,
                per_buffer * 1e6,
                per_buffer / budget * 100.0,
                budget * 1e3,
            );
            if allocs != 0 {
                eprintln!("  !! {allocs} allocations inside process_block — that is a bug");
            }
        }
    }
    println!(
        "\nLatency: {} samples ({:.1} ms at 48 kHz) — two of the longest period the\n\
         detector is sized for, which is what keeps it from moving with the note.",
        AutoTune::new(48_000.0).latency_samples(),
        AutoTune::new(48_000.0).latency_samples() as f32 / 48.0,
    );
}
