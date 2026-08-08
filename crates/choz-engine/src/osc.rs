//! OSC input over UDP: notes plus remote control of the mixer and FX.
//!
//! Same shape as [`crate::midi`]: a background thread parses incoming packets
//! into [`InputEvent`]s and forwards them over a `flume` channel that the UI
//! loop drains, so the RT note ring keeps its single producer.
//!
//! Accepted messages (arguments may be ints or floats; `<tab>` and `<fx>` are
//! 1-based, matching what the UI shows):
//!
//! ```text
//! /note              <note> <velocity>   velocity 0 = note-off
//! /note/on           <note> <velocity>
//! /note/off          <note>
//! /mix/<tab>/gain    <0..1>              1.0 = unity, 2.0 = +6 dB
//! /mix/<tab>/pan     <-1..1>
//! /mix/<tab>/mute    <0|1>
//! /fx/<tab>/<fx>/<param> <0..1>          param is 1-based, as drawn
//! ```

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::input::{ControlMsg, InputEvent, InputSource, NoteMsg};

/// Default UDP port for OSC input.
pub const DEFAULT_PORT: u16 = 9000;

/// A running listener. Dropping it (or calling [`OscHandle::stop`]) shuts the
/// thread down and frees the port, so the settings modal can move OSC to
/// another port without restarting choz.
pub struct OscHandle {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl OscHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Ask the thread to finish. It notices within one socket timeout (200 ms).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for OscHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Bind `0.0.0.0:port` and spawn the listener thread.
pub fn listen(port: u16, tx: flume::Sender<InputEvent>) -> Result<OscHandle> {
    let socket = UdpSocket::bind(("0.0.0.0", port))
        .with_context(|| format!("cannot bind OSC port {port}"))?;
    // A read timeout is what makes the thread stoppable: without it `recv_from`
    // would block forever and never look at the flag.
    socket
        .set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .context("cannot set the OSC socket timeout")?;

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    std::thread::spawn(move || {
        // rosc's max packet size; anything larger is a malformed bundle anyway.
        let mut buf = [0u8; rosc::decoder::MTU];
        while !thread_stop.load(Ordering::Relaxed) {
            let Ok((n, _from)) = socket.recv_from(&mut buf) else { continue };
            let Ok((_rest, packet)) = rosc::decoder::decode_udp(&buf[..n]) else { continue };
            for msg in flatten(packet) {
                let _ = tx.send(msg);
            }
        }
    });
    Ok(OscHandle { port, stop })
}

/// Flatten a packet (message or nested bundle) into input events.
fn flatten(packet: rosc::OscPacket) -> Vec<InputEvent> {
    match packet {
        rosc::OscPacket::Message(m) => parse(&m).into_iter().collect(),
        rosc::OscPacket::Bundle(b) => b.content.into_iter().flat_map(flatten).collect(),
    }
}

/// Parse one OSC message, or `None` if it isn't one we understand.
fn parse(msg: &rosc::OscMessage) -> Option<InputEvent> {
    let num = |i: usize| -> Option<f32> {
        match msg.args.get(i)? {
            rosc::OscType::Int(v) => Some(*v as f32),
            rosc::OscType::Long(v) => Some(*v as f32),
            rosc::OscType::Float(v) => Some(*v),
            rosc::OscType::Double(v) => Some(*v as f32),
            rosc::OscType::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    };
    let key = |i: usize| num(i).map(|v| v.clamp(0.0, 127.0) as u8);
    let note = |on: bool, note: u8, vel: u8| {
        InputEvent::Note(NoteMsg { source: InputSource::Osc, channel: 0, on, note, vel })
    };

    let addr = msg.addr.trim_end_matches('/');
    match addr {
        "/note" | "/note/on" => {
            let k = key(0)?;
            let vel = key(1).unwrap_or(100);
            return Some(note(vel > 0, k, vel));
        }
        "/note/off" => return Some(note(false, key(0)?, 0)),
        _ => {}
    }

    // Control addresses carry their targets in the path: /mix/<tab>/<what> and
    // /fx/<tab>/<fx>/<param>. A 0 index is invalid — these are 1-based.
    let parts: Vec<&str> = addr.trim_start_matches('/').split('/').collect();
    let idx = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
    let value = num(0)?;
    match parts.as_slice() {
        ["mix", tab, what] => {
            let tab = idx(tab)?;
            Some(InputEvent::Control(match *what {
                "gain" => ControlMsg::Gain { tab, value: value.clamp(0.0, 2.0) },
                "pan" => ControlMsg::Pan { tab, value: value.clamp(-1.0, 1.0) },
                "mute" => ControlMsg::Mute { tab, on: value >= 0.5 },
                _ => return None,
            }))
        }
        ["fx", tab, fx, param] => Some(InputEvent::Control(ControlMsg::FxParam {
            tab: idx(tab)?,
            fx: idx(fx)?,
            param: idx(param)?,
            value: value.clamp(0.0, 1.0),
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscMessage, OscType};

    fn osc_note(on: bool, note: u8, vel: u8) -> InputEvent {
        InputEvent::Note(NoteMsg { source: InputSource::Osc, channel: 0, on, note, vel })
    }

    fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
        OscMessage { addr: addr.to_string(), args }
    }

    #[test]
    fn parses_note_messages() {
        assert_eq!(parse(&msg("/note", vec![OscType::Int(60), OscType::Int(90)])), Some(osc_note(true, 60, 90)));
        assert_eq!(parse(&msg("/note", vec![OscType::Int(60), OscType::Int(0)])), Some(osc_note(false, 60, 0)),
            "velocity 0 is a note-off");
        assert_eq!(parse(&msg("/note/on", vec![OscType::Float(60.0)])), Some(osc_note(true, 60, 100)),
            "velocity defaults to 100");
        assert_eq!(parse(&msg("/note/off", vec![OscType::Int(60)])), Some(osc_note(false, 60, 0)));
        assert_eq!(parse(&msg("/note", vec![OscType::Int(200)])), Some(osc_note(true, 127, 100)),
            "out-of-range note clamped");
        assert_eq!(parse(&msg("/note", vec![])), None, "no note argument");
    }

    #[test]
    fn parses_control_messages() {
        use ControlMsg::*;
        let c = |m| Some(InputEvent::Control(m));
        assert_eq!(parse(&msg("/mix/2/gain", vec![OscType::Float(0.5)])), c(Gain { tab: 2, value: 0.5 }));
        assert_eq!(parse(&msg("/mix/1/gain", vec![OscType::Float(9.0)])), c(Gain { tab: 1, value: 2.0 }),
            "gain clamped to the UI's own maximum");
        assert_eq!(parse(&msg("/mix/1/pan", vec![OscType::Float(-2.0)])), c(Pan { tab: 1, value: -1.0 }));
        assert_eq!(parse(&msg("/mix/3/mute", vec![OscType::Int(1)])), c(Mute { tab: 3, on: true }));
        assert_eq!(parse(&msg("/mix/3/mute", vec![OscType::Bool(false)])), c(Mute { tab: 3, on: false }));
        assert_eq!(
            parse(&msg("/fx/1/2/3", vec![OscType::Float(0.25)])),
            c(FxParam { tab: 1, fx: 2, param: 3, value: 0.25 }),
        );
        assert_eq!(parse(&msg("/mix/0/gain", vec![OscType::Float(0.5)])), None, "indices are 1-based");
        assert_eq!(parse(&msg("/mix/2/wobble", vec![OscType::Float(0.5)])), None);
        assert_eq!(parse(&msg("/fx/1/2/3", vec![])), None, "no value");
        assert_eq!(parse(&msg("/nope", vec![OscType::Float(1.0)])), None);
    }

    #[test]
    fn flattens_bundles() {
        let bundle = rosc::OscPacket::Bundle(rosc::OscBundle {
            timetag: rosc::OscTime { seconds: 0, fractional: 0 },
            content: vec![
                rosc::OscPacket::Message(msg("/note", vec![OscType::Int(60), OscType::Int(90)])),
                rosc::OscPacket::Message(msg("/note/off", vec![OscType::Int(62)])),
            ],
        });
        assert_eq!(flatten(bundle), vec![osc_note(true, 60, 90), osc_note(false, 62, 0)]);
    }
}
