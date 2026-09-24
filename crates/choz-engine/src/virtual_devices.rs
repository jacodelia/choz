//! Two devices choz adds to the system while it runs, so it can be used as a
//! multi-effect for everything else without anybody patching anything:
//!
//! * **`choz Mic`** (`choz_mic`) — a virtual *microphone*. It carries choz's
//!   main mix, the same thing that is in the headphones, and Meet, Zoom, Teams
//!   or anything else that asks for a microphone can pick it.
//! * **`choz System FX`** (`choz_system`) — a virtual *speaker*. Whatever the
//!   system plays into it arrives on choz's inputs (its monitor ports are the
//!   one monitor choz lists), so a tab can put effects on it.
//!
//! They are PipeWire null sinks, made through the PulseAudio layer (`pactl`),
//! which is where the applications that need to see them look. Made when the
//! native JACK client starts, taken away when choz closes. A pair left behind
//! by a choz that crashed is found and reused rather than doubled.
//!
//! ponytail: shells out to `pactl` instead of linking libpulse — two commands
//! at start and two at exit. `CHOZ_NO_VIRTUAL_DEVICES=1` turns it off.

use std::process::Command;

/// The virtual microphone's node name: its JACK ports are `choz_mic:input_FL`
/// and `choz_mic:input_FR`.
pub const MIC: &str = "choz_mic";
/// The virtual speaker's node name: its monitor is `choz_system:monitor_FL/FR`.
pub const SYSTEM: &str = "choz_system";

/// The names the **JACK** side knows them by. PipeWire's JACK layer names a
/// client after the node's description, not its node name — `choz Mic:input_FL`,
/// not `choz_mic:input_FL` — and wiring against the node name found nothing,
/// silently: the microphone never got the mix.
pub const MIC_JACK: &str = "choz Mic";
pub const SYSTEM_JACK: &str = "choz System FX";

/// Whether a JACK client or device name is one of these two. Neither is
/// somewhere choz's own output can usefully go: into `choz System FX` is a
/// speaker nobody hears, and into `choz Mic` is what `out_1/2` already do.
pub fn is_ours(name: &str) -> bool {
    [MIC, SYSTEM, MIC_JACK, SYSTEM_JACK].contains(&name)
}

/// The devices this choz made, and so must take away. One made by an earlier
/// run is reused and left alone — it is somebody's, and it costs nothing.
#[derive(Debug, Default)]
pub struct VirtualDevices {
    modules: Vec<u32>,
}

impl VirtualDevices {
    /// Make whatever of the two is missing. Never fails: a system without
    /// `pactl`, or one that says no, just runs choz without them — and says so.
    pub fn ensure() -> Self {
        let mut made = Self::default();
        if std::env::var_os("CHOZ_NO_VIRTUAL_DEVICES").is_some() {
            return made;
        }
        let existing = match pactl(&["list", "short", "sinks"]) {
            Some(sinks) => sinks + &pactl(&["list", "short", "sources"]).unwrap_or_default(),
            None => {
                eprintln!("choz: pactl not found — no 'choz Mic' / 'choz System FX' devices");
                return made;
            }
        };
        let has = |name: &str| {
            existing
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some(name))
        };
        for (name, class, description) in [
            (MIC, Some("Audio/Source/Virtual"), MIC_JACK),
            (SYSTEM, None, SYSTEM_JACK),
        ] {
            if has(name) {
                continue;
            }
            let mut args = vec![
                "load-module".to_string(),
                "module-null-sink".to_string(),
                format!("sink_name={name}"),
                "channel_map=front-left,front-right".to_string(),
                // The whole value quoted, the name inside it in single quotes:
                // quoting only the name let the space end the argument, which
                // dropped the name *and* every argument after it — the
                // microphone came out as a speaker called "choz".
                format!("sink_properties=\"device.description='{description}'\""),
            ];
            if let Some(class) = class {
                args.push(format!("media.class={class}"));
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match pactl(&args).and_then(|id| id.trim().parse::<u32>().ok()) {
                Some(id) => {
                    eprintln!("choz: added the '{description}' device");
                    made.modules.push(id);
                }
                None => eprintln!("choz: could not add the '{description}' device"),
            }
        }
        made
    }
}

impl Drop for VirtualDevices {
    fn drop(&mut self) {
        for id in self.modules.drain(..) {
            let _ = pactl(&["unload-module", &id.to_string()]);
        }
    }
}

/// Run `pactl`, returning its output when it succeeded.
fn pactl(args: &[&str]) -> Option<String> {
    let out = Command::new("pactl").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_names_of_each_device_are_ours() {
        for n in ["choz_mic", "choz_system", "choz Mic", "choz System FX"] {
            assert!(is_ours(n), "{n}");
        }
        assert!(!is_ours("Headset H340 Estéreo analógico"));
    }

    /// Against the real system, so not in the normal run: the two devices
    /// appear, and go when the handle does.
    /// `cargo test -p choz-engine --lib virtual_devices -- --ignored`
    #[test]
    #[ignore]
    fn the_devices_come_and_go_with_the_handle() {
        let listed = |name: &str| {
            let all = pactl(&["list", "short", "sinks"]).unwrap_or_default()
                + &pactl(&["list", "short", "sources"]).unwrap_or_default();
            all.lines()
                .any(|l| l.split_whitespace().nth(1) == Some(name))
        };
        let made = VirtualDevices::ensure();
        assert!(listed(MIC) && listed(SYSTEM), "both devices exist");
        // The microphone is a source (not a speaker with a monitor), under
        // the name a meeting app shows.
        let sources = Command::new("pactl")
            .args(["list", "sources"])
            .env("LC_ALL", "C")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        assert!(sources.contains("Name: choz_mic\n"), "choz_mic is a source");
        assert!(
            sources.contains("Description: choz Mic"),
            "named 'choz Mic'"
        );
        let again = VirtualDevices::ensure();
        assert!(again.modules.is_empty(), "a second run reuses them");
        drop(again);
        assert!(listed(MIC), "and does not take away what it did not make");
        // And the JACK side names them by their description — the names the
        // wiring uses. Assuming the node names wired nothing, silently.
        if let Ok((client, _)) =
            jack::Client::new("choz-vd-test", jack::ClientOptions::NO_START_SERVER)
        {
            let ports = client.ports(None, None, jack::PortFlags::empty());
            for want in [
                format!("{MIC_JACK}:input_FL"),
                format!("{SYSTEM_JACK}:monitor_FL"),
            ] {
                assert!(ports.contains(&want), "no JACK port {want}");
            }
        }
        let ours = !made.modules.is_empty();
        drop(made);
        if ours {
            assert!(!listed(MIC) && !listed(SYSTEM), "gone with the handle");
        }
    }
}
