//! The handshake over real shared memory, between real processes.
//!
//! Custom harness (`harness = false`): the test re-runs itself as the child,
//! the same trick the scan and probe workers use.

use std::time::Duration;

use choz_plugin_sandbox::shm::Shm;
use choz_plugin_sandbox::{Host, Sandbox, region_bytes};

const FRAMES: u32 = 64;
const CHANNELS: u32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--child" {
        child(&args[2], args.get(3).is_some_and(|a| a == "--crash"));
        return;
    }

    let name = choz_plugin_sandbox::shm::unique_name("test");
    let bytes = region_bytes(FRAMES, CHANNELS);
    let mut shm = Shm::create(&name, bytes).expect("create shm");
    let mut host = unsafe { Host::create(shm.as_ptr(), FRAMES, CHANNELS, 48_000) };

    let exe = std::env::current_exe().expect("current_exe");
    let mut kid = std::process::Command::new(exe)
        .arg("--child")
        .arg(&name)
        .spawn()
        .expect("spawn child");

    let input = vec![0.25f32; (FRAMES * CHANNELS) as usize];
    let mut output = vec![0.0f32; (FRAMES * CHANNELS) as usize];

    // The first exchange also covers the child's startup, so it gets longer.
    host.push_midi([0x90, 60, 100]);
    assert!(
        host.exchange(&input, &mut output, Duration::from_secs(5)),
        "the child never answered the first block"
    );
    // Now the name can go: both mappings are live and nothing can leak it.
    shm.unlink();
    assert!(
        output.iter().all(|s| (*s - 0.5).abs() < 1e-6),
        "the child should have doubled the block: {:?}",
        &output[..4]
    );

    // …and it keeps up, block after block, well inside a buffer period.
    for i in 0..200 {
        let level = (i % 8) as f32 / 8.0;
        let block = vec![level; input.len()];
        assert!(
            host.exchange(&block, &mut output, Duration::from_millis(20)),
            "child missed block {i}"
        );
        assert!(
            output.iter().all(|s| (*s - level * 2.0).abs() < 1e-6),
            "block {i} came back wrong"
        );
    }
    assert_eq!(host.missed(), 0, "no block should have been dropped");

    host.stop();
    let status = kid.wait().expect("child exit");
    assert!(status.success(), "child exited with {status}");
    println!("test one_block_out_one_block_back_across_two_processes ... ok");

    a_dying_child_costs_a_glitch();
}

/// The reason the whole thing exists: a plugin that segfaults mid-stream must
/// cost silent blocks, not the process.
fn a_dying_child_costs_a_glitch() {
    let name = choz_plugin_sandbox::shm::unique_name("crash");
    let mut shm = Shm::create(&name, region_bytes(FRAMES, CHANNELS)).expect("create shm");
    let mut host = unsafe { Host::create(shm.as_ptr(), FRAMES, CHANNELS, 48_000) };
    let exe = std::env::current_exe().expect("current_exe");
    let mut kid = std::process::Command::new(exe)
        .arg("--child")
        .arg(&name)
        .arg("--crash")
        .spawn()
        .expect("spawn child");

    let input = vec![0.5f32; (FRAMES * CHANNELS) as usize];
    let mut output = vec![0.0f32; (FRAMES * CHANNELS) as usize];
    assert!(host.exchange(&input, &mut output, Duration::from_secs(5)), "first block");
    shm.unlink();

    // The child dies on its second block. Everything after is silence, and the
    // host is still here to notice.
    let mut silent = 0;
    for _ in 0..5 {
        if !host.exchange(&input, &mut output, Duration::from_millis(50)) {
            silent += 1;
            assert!(output.iter().all(|s| *s == 0.0), "a missed block must be silent");
        }
    }
    assert!(silent > 0, "the dead child should have missed blocks");
    assert_eq!(host.missed(), silent);

    let status = kid.wait().expect("child exit");
    assert!(!status.success(), "the child was supposed to die: {status}");
    println!("test a_plugin_that_dies_mid_stream_costs_silence ... ok");
}

/// The plugin's side, standing in for a hosted plugin: double every sample.
fn child(name: &str, crash: bool) {
    let shm = Shm::attach(name, region_bytes(FRAMES, CHANNELS)).expect("attach shm");
    let mut sandbox = unsafe { Sandbox::attach(shm.as_ptr(), FRAMES, CHANNELS) };
    assert_eq!(sandbox.sample_rate(), 48_000, "the header crossed intact");

    let mut served = 0;
    while sandbox.serve(Duration::from_secs(5), &mut |input, output, _midi, _params| {
        for (o, i) in output.iter_mut().zip(input) {
            *o = i * 2.0;
        }
    }) {
        served += 1;
        if crash && served >= 1 {
            // What a broken plugin does, minus the wait for one to misbehave.
            std::process::abort();
        }
    }
}
