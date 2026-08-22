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
pub(crate) const MAX_PORTS: usize = 32;

/// The RT side: the shared engine state plus this client's ports.
pub struct JackRt {
    state: RtState,
    out: Vec<Port<AudioOut>>,
    inp: Vec<Port<AudioIn>>,
}

impl jack::ProcessHandler for JackRt {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let started = std::time::Instant::now();
        let cpu_started = crate::meter::cpu_micros();
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
        crate::engine::publish_load(started, cpu_started, frames, self.state.sample_rate);
        Control::Continue
    }
}

/// A live JACK client. Dropping it deactivates and unregisters everything.
pub type Handle = AsyncClient<(), JackRt>;

/// The rate the graph is actually running at. `None` when it can't be reached.
///
/// This is a **fact**, unlike the rate saved in the settings, which is a wish:
/// PipeWire hands every JACK client the graph's rate and nothing negotiates it
/// per client. Asking before the engine builds anything is the only way to
/// build it for the right rate.
pub fn graph_rate() -> Option<u32> {
    let (client, _) = Client::new("choz-probe", ClientOptions::NO_START_SERVER).ok()?;
    Some(client.sample_rate() as u32)
}

/// Playback/capture port counts of `sink` as the graph currently publishes it.
/// `None` when the graph can't be reached; `(0, 0)` when the name is unknown.
pub fn device_channels(sink: &str) -> Option<(usize, usize)> {
    let (client, _) = Client::new("choz-probe", ClientOptions::NO_START_SERVER).ok()?;
    Some((
        sink_ports(&client, sink).len(),
        source_ports(&client, sink).len(),
    ))
}

/// **Every** capture port in the graph, ours excluded — an interface's eight
/// inputs *and* the laptop's microphone *and* the other card, grouped by the
/// node that owns them and channel-ordered inside each group.
///
/// This is what the IN drawer lists. Taking the capture ports of the *sink*
/// instead (which is what choz used to do) finds nothing at all on a PipeWire
/// box, because playback and capture of one interface are two separate nodes:
/// an eight-input UMC1820 showed `AUDIO IN (0)`.
pub fn all_capture_ports() -> Vec<String> {
    let Ok((client, _)) = Client::new("choz-probe", ClientOptions::NO_START_SERVER) else {
        return Vec::new();
    };
    let mut owners: Vec<String> = Vec::new();
    for port in client.ports(
        None,
        Some(super::engine::JACK_AUDIO),
        jack::PortFlags::IS_OUTPUT,
    ) {
        // `monitor_*` carries back what we just played, and our own ports would
        // feed the rack into itself.
        if port.contains(":monitor") {
            continue;
        }
        let Some((owner, _)) = port.rsplit_once(':') else {
            continue;
        };
        if owner == CLIENT_NAME || owner == super::engine::CPAL_JACK_CLIENT {
            continue;
        }
        if !owners.iter().any(|o| o == owner) {
            owners.push(owner.to_string());
        }
    }
    owners
        .iter()
        .flat_map(|owner| source_ports(&client, owner))
        .take(MAX_PORTS)
        .collect()
}

/// The device's playback ports — where our audio goes — in channel order.
pub(crate) fn sink_ports(client: &Client, sink: &str) -> Vec<String> {
    let prefix = format!("{sink}:");
    in_order(
        client
            .ports(
                None,
                Some(super::engine::JACK_AUDIO),
                jack::PortFlags::IS_INPUT,
            )
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
            .ports(
                None,
                Some(super::engine::JACK_AUDIO),
                jack::PortFlags::IS_OUTPUT,
            )
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
        digits
            .chars()
            .rev()
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    ports
}

/// What the audio ended up connected to: the sink's name and how many of our
/// output ports reached it.
pub type Wiring = Option<(String, usize)>;

/// Open the client, register the ports, wire them to `sink` when one is named,
/// and start processing. Returns the live client, the number of output ports
/// registered, and — when a sink was asked for — which one the audio actually
/// ended up going to and how many of our ports reached it.
///
/// `capture` is the graph ports our inputs are wired to, one for one — see
/// [`all_capture_ports`]. Its length *is* the input channel count.
pub fn start(
    sink: Option<&str>,
    capture: &[String],
    outs: usize,
    state: RtState,
) -> Result<(Handle, usize, Wiring)> {
    let ins = capture.len();
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
    let mut wired_to = None;
    if let Some(sink) = sink {
        match connect(handle.as_client(), &our_outs, sink) {
            Ok((name, wired)) => wired_to = Some((name, wired)),
            Err(e) => eprintln!("choz: {e}"),
        }
    }
    // Capture: every input jack in the graph, wired one for one. A device that
    // vanished between the scan and here just fails to connect — but **say
    // so**. Swallowing these is how choz ends up with input ports wired to
    // nothing, which looks exactly like a broken effect: the rows are there,
    // the routing is there, and the signal never arrives.
    let mut failed = 0usize;
    for (from, ours) in capture.iter().zip(our_ins.iter()) {
        if let Err(e) = handle.as_client().connect_ports_by_name(from, ours) {
            if failed < 3 {
                eprintln!("choz: cannot wire capture '{from}' -> '{ours}': {e}");
            }
            failed += 1;
        }
    }
    if failed > 0 {
        eprintln!(
            "choz: {failed} of {} capture ports could not be wired; those channels will be silent",
            capture.len()
        );
    }
    Ok((handle, outs, wired_to))
}

/// Wire our outputs to `sink`, channel for channel: `out_1` → the sink's first
/// playback port.
///
/// **Falls back to a sink that exists.** The saved output is a name, and a
/// name outlives the box it belonged to: an interface switched off since last
/// time leaves choz wired to nothing, which is silence that looks exactly like
/// a broken effect. Returns the sink actually wired to and how many ports were
/// joined, so the caller can say which one it ended up on.
fn connect(client: &Client, our_outs: &[String], sink: &str) -> Result<(String, usize)> {
    let name = sink.to_string();
    let targets = sink_ports(client, sink);
    if targets.is_empty() {
        // **Nothing automatic.** choz used to pick another sink when the chosen
        // one had no ports, which meant an interface being off moved the whole
        // rig to the laptop speakers without being asked — mid-set, and audibly.
        // The device is the user's choice and only the user changes it: this
        // stays unconnected, the TRANSPORT panel says `NOT CONNECTED`, and the
        // OUT drawer is where it gets picked again.
        eprintln!(
            "choz: output '{sink}' has no playback ports — staying unconnected. \
             Pick one in the OUT drawer (F3), or 'r' there to rescan."
        );
        return Ok((name, 0));
    }
    let mut wired = 0usize;
    for (ours, target) in our_outs.iter().zip(targets.iter()) {
        // Drop whatever the graph auto-connected us to, or the same audio
        // reaches two sinks.
        if let Some(port) = client.port_by_name(ours) {
            for old in port.get_connections() {
                let _ = client.disconnect_ports_by_name(ours, &old);
            }
        }
        match client.connect_ports_by_name(ours, target) {
            Ok(()) => wired += 1,
            Err(e) => eprintln!("choz: cannot wire '{ours}' -> '{target}': {e}"),
        }
    }
    Ok((name, wired))
}
