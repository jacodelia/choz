//! What the reverb costs, in the only unit that matters: how much of one core
//! one instance uses to keep up with the audio clock.
//!
//! Run with `cargo run --release --example reverb_bench`. In a debug build the
//! numbers mean nothing at all — the interpolators and the filters are the sort
//! of arithmetic that is four times slower unoptimised — so it says so if it is
//! not in release.
//!
//! It also proves the allocation claim rather than asserting it: a counting
//! allocator wraps the global one, and the count is read either side of the
//! processing loop. Anything but zero is a bug, not a slow path.

use choz_engine::fx::reverb::{Quality, Reverb};
use choz_engine::fx::FxProcessor;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
/// Sixty seconds of audio through each configuration: long enough that the
/// measurement is of the reverb and not of the timer.
const SECONDS: usize = 60;

fn bench(name: &str, quality: Quality, block: usize) {
    let mut r = Reverb::new(SR);
    r.set_quality(quality);
    r.set_mix(0.4);
    r.set_decay(0.7);

    let mut buf = vec![0.0f32; block * 2];
    let mut seed = 0x2545_F491u32;
    let blocks = SECONDS * SR as usize / block;

    // Warm the caches and let every smoother arrive, so the timed run is the
    // steady state rather than the first two hundred milliseconds of it.
    for _ in 0..(SR as usize / block) {
        r.process_block(&mut buf, SR);
    }

    let before = ALLOCS.load(Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..blocks {
        for s in buf.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s = (seed >> 8) as f32 / (1 << 23) as f32 - 1.0;
        }
        r.process_block(&mut buf, SR);
    }
    let took = start.elapsed();
    let allocs = ALLOCS.load(Ordering::Relaxed) - before;

    let audio = SECONDS as f64;
    let load = took.as_secs_f64() / audio;
    println!(
        "  {name:<22} {:>7.3}% of a core   ×{:>6.0} realtime   {:>5} allocations",
        load * 100.0,
        1.0 / load,
        allocs
    );
    assert_eq!(allocs, 0, "{name} allocated inside process_block");
}

fn main() {
    if choz_engine::worker_main() {
        return;
    }
    if cfg!(debug_assertions) {
        println!("\n  ** debug build — these numbers are meaningless **");
        println!("  run: cargo run --release --example reverb_bench\n");
    }
    println!("\nchoz reverb — {SECONDS}s of stereo at {SR} Hz, one instance\n");
    bench("Economy (4×4 FDN)", Quality::Economy, BLOCK);
    bench("High (8×8 FDN)", Quality::High, BLOCK);

    println!("\nthe same, by block size — the cost must not depend on it\n");
    for block in [32usize, 128, 512, 2048] {
        bench(&format!("High, {block} frames"), Quality::High, block);
    }

    // What one instance holds. Every byte of it is taken in `new`.
    let bytes = std::mem::size_of::<Reverb>();
    println!("\n  struct {bytes} B, and the delay buffers it allocates once at construction:");
    let before = ALLOCS.load(Ordering::Relaxed);
    let mark = Instant::now();
    let r = Reverb::new(SR);
    println!(
        "  {} allocations in new(), {:.1} ms\n",
        ALLOCS.load(Ordering::Relaxed) - before,
        mark.elapsed().as_secs_f64() * 1e3
    );
    drop(r);
}
