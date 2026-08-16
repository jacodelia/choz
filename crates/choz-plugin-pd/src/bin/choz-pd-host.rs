//! One Pure Data patch, in a process of its own.
//!
//! ```text
//! choz-pd-host <patch.pd> <shm-name> <frames>
//! ```
//!
//! This is the only binary that links libpd, and that is deliberate: libpd is
//! LGPL and choz is MIT, so the linking stays on this side of the process
//! boundary. It is also the only shape that *works* — Debian's libpd has no
//! `PDINSTANCE`, so a process holds exactly one Pd, and two patches in one choz
//! would silently share a DSP graph.
//!
//! Audio crosses through [`choz_plugin_sandbox`], the same shared region the
//! plugin sandbox uses, so the host end (supervisor, restarts, deadline,
//! silence when a block is late) is code that already exists and already works.
//!
//! Built only with `--features pd`, because it needs libpd to link at all.

use std::path::Path;
use std::time::Duration;

use choz_plugin_sandbox::shm::Shm;
use choz_plugin_sandbox::{region_bytes, Sandbox};

/// Interleaved stereo, like every other block in choz.
const CHANNELS: u32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: choz-pd-host <patch.pd> <shm-name> <frames>");
        std::process::exit(2);
    }
    let patch = Path::new(&args[1]);
    let shm_name = args[2].as_str();
    let frames: u32 = args[3].parse().unwrap_or(256);

    if let Err(e) = serve(patch, shm_name, frames) {
        eprintln!("choz-pd-host: {}: {e}", patch.display());
        std::process::exit(1);
    }
}

fn serve(patch: &Path, shm_name: &str, frames: u32) -> anyhow::Result<()> {
    let shm = Shm::attach(shm_name, region_bytes(frames, CHANNELS))?;
    // SAFETY: same size and layout the host created, which is what
    // `region_bytes` with the host's frames/channels means.
    let mut sandbox = unsafe { Sandbox::attach(shm.as_ptr(), frames, CHANNELS) };
    let sample_rate = sandbox.sample_rate();

    // Opening the patch is what allocates in Pd; after this it does not. So it
    // happens here, before the first block is served, exactly like the plugin
    // sandbox loads its plugin before answering.
    let mut patch = choz_plugin_pd::Patch::open(patch, sample_rate)?;
    // A patch has no window choz can embed, so say so rather than leaving the
    // host offering a `GUI` button for one that will never open. Pd's own
    // canvas is a different program.
    sandbox.set_editor_present(false);

    while sandbox.serve(Duration::from_secs(5), &mut |input, output, _midi, params| {
        // The knobs the interface is showing, by the same index it shows them
        // in — both sides ask `choz_plugin_pd::addressable` for the order.
        for (index, value) in params {
            patch.set_control(*index, *value);
        }
        // Pd processes out of place; the block arrives in `input` and the
        // patch's answer is what the host reads back.
        output.copy_from_slice(input);
        patch.process(output);
    }) {}
    Ok(())
}
