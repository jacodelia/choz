//! Finding the OSC server a plugin opened inside this process, and pinging it.
//!
//! ZynAddSubFX's LV2 runs a liblo server and its editor is a **separate
//! program** (`zynaddsubfx-ext-gui`) that talks to it — which is why its own
//! `ui:showInterface` puts nothing on screen here: DPF hands the UI the address
//! over an atom port choz does not implement, so the UI never learns where to
//! connect and never starts the program.
//!
//! The address is not in the plugin's state, nor in any LV2 interface: it is
//! chosen at instantiate and printed to stdout. But the socket is opened **by
//! this process**, so it can be found from the outside: take the process's UDP
//! ports before and after loading the plugin, and the new one is its server.
//! Confirmed by asking it something only an rtosc server answers, so a port
//! that some other library happened to open is never mistaken for one.

use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

/// Every UDP port this process has a socket bound to.
///
/// Linux-only, like the editor window itself. An empty set (a kernel without
/// `/proc`, a sandbox that hides it) simply means nothing is ever found, and
/// the caller falls back to having no editor — never to a wrong one.
pub fn udp_ports() -> HashSet<u16> {
    let mut ours: HashSet<u64> = HashSet::new();
    let Ok(fds) = std::fs::read_dir("/proc/self/fd") else {
        return HashSet::new();
    };
    for fd in fds.flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else {
            continue;
        };
        // "socket:[12345]"
        if let Some(inode) = target
            .to_string_lossy()
            .strip_prefix("socket:[")
            .and_then(|s| s.strip_suffix(']'))
            .and_then(|s| s.parse::<u64>().ok())
        {
            ours.insert(inode);
        }
    }

    let mut out = HashSet::new();
    for table in ["/proc/net/udp", "/proc/net/udp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            // sl local rem st tx:rx tr:tm retrnsmt uid timeout inode
            let (Some(local), Some(inode)) = (f.get(1), f.get(9)) else {
                continue;
            };
            let Ok(inode) = inode.parse::<u64>() else {
                continue;
            };
            if !ours.contains(&inode) {
                continue;
            }
            if let Some(port) = local
                .rsplit(':')
                .next()
                .and_then(|p| u16::from_str_radix(p, 16).ok())
            {
                out.insert(port);
            }
        }
    }
    out
}

/// The rtosc server that appeared since `before` was taken, if any.
pub fn server_since(before: &HashSet<u16>) -> Option<u16> {
    udp_ports()
        .into_iter()
        .filter(|p| !before.contains(p))
        .find(|p| is_rtosc(*p))
}

/// Whether `port` answers `/path-search`, which an rtosc server does and an
/// unrelated UDP socket does not.
fn is_rtosc(port: u16) -> bool {
    let Ok(sock) = UdpSocket::bind("127.0.0.1:0") else {
        return false;
    };
    if sock
        .set_read_timeout(Some(Duration::from_millis(300)))
        .is_err()
    {
        return false;
    }
    let msg = message("/path-search", &[Arg::Str("/"), Arg::Str("")]);
    if sock.send_to(&msg, ("127.0.0.1", port)).is_err() {
        return false;
    }
    let mut buf = [0u8; 2048];
    match sock.recv_from(&mut buf) {
        Ok((n, _)) => n > 0 && buf.starts_with(b"/paths"),
        Err(_) => false,
    }
}

/// The arguments an OSC message can carry here. Only what is needed to ask a
/// question and to move a parameter.
pub enum Arg<'a> {
    Int(i32),
    Float(f32),
    Str(&'a str),
    /// OSC's own true/false: the type tag *is* the value, so nothing follows it
    /// in the body. Zyn's switches are these, not integers.
    Bool(bool),
}

/// One OSC message, encoded. Addresses and strings are NUL-terminated and
/// padded to four bytes; that is the whole of the wire format used here.
pub fn message(address: &str, args: &[Arg]) -> Vec<u8> {
    let mut out = Vec::new();
    push_str(&mut out, address);
    let mut tags = String::from(",");
    for a in args {
        tags.push(match a {
            Arg::Int(_) => 'i',
            Arg::Float(_) => 'f',
            Arg::Str(_) => 's',
            Arg::Bool(true) => 'T',
            Arg::Bool(false) => 'F',
        });
    }
    push_str(&mut out, &tags);
    for a in args {
        match a {
            Arg::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Str(v) => push_str(&mut out, v),
            // Its tag is the whole of it.
            Arg::Bool(_) => {}
        }
    }
    out
}

/// The address and first argument of an OSC message, as far as anything here
/// needs to read one: a value coming back from the plugin.
///
/// `None` for a message this does not understand, which is thrown away rather
/// than guessed at.
pub fn read_value(data: &[u8]) -> Option<(String, f32)> {
    let end = data.iter().position(|b| *b == 0)?;
    let address = std::str::from_utf8(&data[..end]).ok()?.to_string();
    let i = (end / 4 + 1) * 4;
    let tags_end = data.get(i..)?.iter().position(|b| *b == 0)? + i;
    let tags = std::str::from_utf8(data.get(i..tags_end)?).ok()?;
    let body = data.get((tags_end / 4 + 1) * 4..)?;
    let four = |b: &[u8]| -> Option<[u8; 4]> { b.get(..4)?.try_into().ok() };
    let value = match tags.chars().nth(1)? {
        // `c` is a character, sent in a whole word like an integer — which is
        // what Zyn's harmonic magnitudes are.
        'i' | 'c' => i32::from_be_bytes(four(body)?) as f32,
        'f' => f32::from_be_bytes(four(body)?),
        'T' => 1.0,
        'F' => 0.0,
        _ => return None,
    };
    Some((address, value))
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    // At least one NUL, then up to the next multiple of four.
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

// ─── The client ─────────────────────────────────────────────────────────────

/// A socket to one plugin's OSC server, with the values it has answered.
///
/// rtosc answers **when it gets round to it**: a read is a message out and a
/// message back some milliseconds later, in no particular order with respect to
/// anything else asked meanwhile. So replies are collected by a thread of their
/// own into a table the UI reads whenever it draws, and nothing ever waits.
pub struct OscClient {
    sock: Arc<UdpSocket>,
    port: u16,
    values: Arc<Mutex<HashMap<String, f32>>>,
    alive: Arc<AtomicBool>,
}

impl OscClient {
    /// Talk to the server on `port`, and start listening for what it says back.
    pub fn connect(port: u16) -> Option<Self> {
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").ok()?);
        // So the reader thread notices the flag instead of blocking for ever.
        sock.set_read_timeout(Some(Duration::from_millis(200)))
            .ok()?;
        let values: Arc<Mutex<HashMap<String, f32>>> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (sock, values, alive) =
                (Arc::clone(&sock), Arc::clone(&values), Arc::clone(&alive));
            std::thread::Builder::new()
                .name("choz-lv2-osc".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 65536];
                    while alive.load(Ordering::Relaxed) {
                        let Ok((n, _)) = sock.recv_from(&mut buf) else {
                            continue; // timeout, or the socket went away
                        };
                        if let Some((address, value)) = read_value(&buf[..n]) {
                            values.lock().insert(address, value);
                        }
                    }
                })
                .ok()?;
        }
        let me = Self {
            sock,
            port,
            values,
            alive,
        };
        // Tell the plugin where to answer. Zyn's middleware replies to whoever
        // asked only for what it holds itself; anything it forwards to the
        // audio side — every harmonic of an oscillator — is answered to the
        // address registered here, which is what its own editor registers on
        // startup. Without it those reads are silence.
        if let Ok(local) = me.sock.local_addr() {
            me.send(
                "/echo",
                &[
                    Arg::Str("OSC_URL"),
                    Arg::Str(&format!("osc.udp://127.0.0.1:{}/", local.port())),
                ],
            );
        }
        Some(me)
    }

    /// Send a message. Fire and forget: a plugin that is not listening is a
    /// knob that does nothing, never a stall.
    pub fn send(&self, address: &str, args: &[Arg]) {
        let _ = self
            .sock
            .send_to(&message(address, args), ("127.0.0.1", self.port));
    }

    /// Ask what a path holds. The answer arrives in [`Self::value`] later.
    pub fn ask(&self, address: &str) {
        self.send(address, &[]);
    }

    /// The last value the plugin reported for `address`, if it has.
    pub fn value(&self, address: &str) -> Option<f32> {
        self.values.lock().get(address).copied()
    }
}

impl Drop for OscClient {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder against the shape the spec fixes: address and type tag both
    /// NUL-terminated and four-byte aligned, integers big-endian.
    #[test]
    fn a_message_is_padded_the_way_osc_says() {
        let m = message("/part0/Ppanning", &[Arg::Int(100)]);
        assert_eq!(&m[..16], b"/part0/Ppanning\0");
        assert_eq!(&m[16..20], b",i\0\0");
        assert_eq!(&m[20..], &100i32.to_be_bytes());
        assert!(m.len().is_multiple_of(4));

        // A four-character address still gets its own NUL and a full pad.
        let m = message("/foo", &[]);
        assert_eq!(&m[..8], b"/foo\0\0\0\0");
        assert_eq!(&m[8..], b",\0\0\0");
    }

    /// Anything can arrive on a socket. A message that is truncated, padded
    /// wrong or simply not one has to come back as `None` — never as an index
    /// off the end of the buffer.
    #[test]
    fn a_malformed_message_is_refused_rather_than_read() {
        let good = message("/part0/Ppanning", &[Arg::Int(100)]);
        assert_eq!(
            read_value(&good),
            Some(("/part0/Ppanning".to_string(), 100.0))
        );

        // Every prefix of a real message, including the empty one.
        for n in 0..good.len() {
            let _ = read_value(&good[..n]);
        }
        // …and a spread of rubbish, none of it a message.
        for seed in 0u16..512 {
            let junk: Vec<u8> = (0..(seed % 37) as u8)
                .map(|i| i.wrapping_mul(seed as u8).wrapping_add(7))
                .collect();
            let _ = read_value(&junk);
        }
        assert_eq!(read_value(&[]), None);
        assert_eq!(
            read_value(b"/no-tags\0\0\0\0"),
            None,
            "no type tag, no value"
        );
    }

    /// A socket this process opened is one this process can find.    /// A socket this process opened is one this process can find.
    #[test]
    fn our_own_udp_socket_is_in_the_list() {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bound");
        let port = sock.local_addr().unwrap().port();
        assert!(
            udp_ports().contains(&port),
            "the port table did not have {port}"
        );
        // …and it is not an rtosc server, so nothing else is mistaken for one.
        assert!(!is_rtosc(port));
    }
}
