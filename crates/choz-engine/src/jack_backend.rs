//! Native JACK client: one port per device channel, one process callback.
//!
//! cpal's JACK host only ever offers a fixed stereo pair, which is why choz
//! opens its own client here: an interface with 12 outputs gets 12 ports, so a
//! slot can be routed to outputs 5/6 while another plays out of 1/2. Capture
//! ports are registered at the same time — the routing UI for them lands in a
//! second pass, but the backend already carries their audio into the slots.

use anyhow::{Context, Result};
use jack::{AsyncClient, AudioIn, AudioOut, Client, ClientOptions, Control, Port, ProcessScope};

use crate::engine::RtState;

/// JACK client name choz registers under. Also what shows up in Carla/qpwgraph.
pub const CLIENT_NAME: &str = "choz";

/// Ceiling on ports we register per direction. Interfaces above this are still
/// usable, just not to their last channels.
/// ponytail: a constant, not a setting — nothing here has more than 32 jacks.
const MAX_PORTS: usize = 32;

/// The RT side: the shared engine state plus this client's ports.
pub struct JackRt {
    state: RtState,
    out: Vec<Port<AudioOut>>,
    inp: Vec<Port<AudioIn>>,
}

impl jack::ProcessHandler for JackRt {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let frames = ps.n_frames() as usize;
        self.state.apply_commands();

        // Hardware inputs first: `render` reads them for any slot routed to a
        // capture pair, and runs that slot's FX chain over the live audio.
        for (i, port) in self.inp.iter().enumerate() {
            self.state.write_capture(i, port.as_slice(ps));
        }

        self.state.render(frames);

        for (i, port) in self.out.iter_mut().enumerate() {
            let buf = port.as_mut_slice(ps);
            let n = frames.min(buf.len());
            match self.state.mix.get(i) {
                Some(src) => buf[..n].copy_from_slice(&src[..n]),
                None => buf[..n].fill(0.0),
            }
        }
        Control::Continue
    }
}

/// A live JACK client. Dropping it deactivates and unregisters everything.
pub type Handle = AsyncClient<(), JackRt>;

/// Playback/capture port counts of `sink` as the graph currently publishes it.
/// `None` when the graph can't be reached; `(0, 0)` when the name is unknown.
pub fn device_channels(sink: &str) -> Option<(usize, usize)> {
    let (client, _) = Client::new("choz-probe", ClientOptions::NO_START_SERVER).ok()?;
    Some((sink_ports(&client, sink).len(), source_ports(&client, sink).len()))
}

/// Capture ports of `source`. A separate input device is the normal case on a
/// plain sound card, where playback (`alsa_output…`) and capture
/// (`alsa_input…`) are two different graph nodes.
pub fn capture_channels(source: &str) -> Option<usize> {
    let (client, _) = Client::new("choz-probe", ClientOptions::NO_START_SERVER).ok()?;
    Some(source_ports(&client, source).len())
}

/// The device's playback ports — where our audio goes — in channel order.
pub(crate) fn sink_ports(client: &Client, sink: &str) -> Vec<String> {
    let prefix = format!("{sink}:");
    in_order(
        client
            .ports(None, Some(super::engine::JACK_AUDIO), jack::PortFlags::IS_INPUT)
            .into_iter()
            .filter(|p| p.starts_with(&prefix))
            .collect(),
    )
}

/// The device's capture ports, in channel order. `monitor_*` is deliberately
/// left out: those carry back what we just played, so feeding them into our
/// inputs would loop the rack through itself.
fn source_ports(client: &Client, sink: &str) -> Vec<String> {
    let prefix = format!("{sink}:");
    in_order(
        client
            .ports(None, Some(super::engine::JACK_AUDIO), jack::PortFlags::IS_OUTPUT)
            .into_iter()
            .filter(|p| p.starts_with(&prefix) && !p.contains(":monitor"))
            .collect(),
    )
}

/// Sort by the number the port name ends in, so channel 10 doesn't sort
/// between 1 and 2 and land the audio on the wrong jack.
pub(crate) fn in_order(mut ports: Vec<String>) -> Vec<String> {
    ports.sort_by_key(|p| {
        let digits: String = p.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
        digits.chars().rev().collect::<String>().parse::<u32>().unwrap_or(u32::MAX)
    });
    ports
}

/// Open the client, register `outs`/`ins` ports, wire them to `sink` when one
/// is named, and start processing. Returns the live client and the number of
/// output ports actually registered.
pub fn start(
    sink: Option<&str>,
    source: Option<&str>,
    outs: usize,
    ins: usize,
    state: RtState,
) -> Result<(Handle, usize)> {
    let (client, _status) = Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER)
        .context("cannot reach the JACK graph (is PipeWire's JACK layer installed?)")?;

    let outs = outs.clamp(2, MAX_PORTS);
    let ins = ins.min(MAX_PORTS);

    let out: Vec<Port<AudioOut>> = (0..outs)
        .map(|i| client.register_port(&format!("out_{}", i + 1), AudioOut))
        .collect::<Result<_, _>>()
        .context("cannot register JACK output ports")?;
    let inp: Vec<Port<AudioIn>> = (0..ins)
        .map(|i| client.register_port(&format!("in_{}", i + 1), AudioIn))
        .collect::<Result<_, _>>()
        .context("cannot register JACK input ports")?;

    let our_outs: Vec<String> = out.iter().filter_map(|p| p.name().ok()).collect();
    let our_ins: Vec<String> = inp.iter().filter_map(|p| p.name().ok()).collect();

    let handle = client
        .activate_async((), JackRt { state, out, inp })
        .map_err(|e| anyhow::anyhow!("cannot activate the JACK client: {e}"))?;

    // Wiring happens after activation: ports only exist in the graph once the
    // client is live. A sink that went away is not fatal — choz still runs,
    // just unconnected, and the user can patch it anywhere.
    if let Some(sink) = sink {
        if let Err(e) = connect(handle.as_client(), &our_outs, sink) {
            eprintln!("choz: {e}");
        }
    }
    // Capture comes from wherever the user pointed it — the same node as the
    // output on a duplex interface, a different one on a plain sound card.
    if let Some(source) = source {
        connect_capture(handle.as_client(), &our_ins, source);
    }
    Ok((handle, outs))
}

/// Wire our outputs to `sink`, channel for channel: `out_1` → the sink's first
/// playback port.
fn connect(client: &Client, our_outs: &[String], sink: &str) -> Result<()> {
    let targets = sink_ports(client, sink);
    if targets.is_empty() {
        anyhow::bail!("output '{sink}' has no playback ports; leaving choz unconnected");
    }
    for (ours, target) in our_outs.iter().zip(targets.iter()) {
        // Drop whatever the graph auto-connected us to, or the same audio
        // reaches two sinks.
        if let Some(port) = client.port_by_name(ours) {
            for old in port.get_connections() {
                let _ = client.disconnect_ports_by_name(ours, &old);
            }
        }
        let _ = client.connect_ports_by_name(ours, target);
    }

    Ok(())
}

/// Wire `source`'s capture ports into our `in_*`, channel for channel.
fn connect_capture(client: &Client, our_ins: &[String], source: &str) {
    for (from, ours) in source_ports(client, source).iter().zip(our_ins.iter()) {
        let _ = client.connect_ports_by_name(from, ours);
    }
}
