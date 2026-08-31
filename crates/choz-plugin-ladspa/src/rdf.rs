//! Step names for a LADSPA port, from the metadata files beside the plugins.
//!
//! The ABI has no call for this: a `LADSPA_Descriptor` says a port is toggled
//! or integral and how far it runs, and nothing at all about what any of its
//! positions *mean*. So "waveform" is four numbers and a switch is a number
//! that happens to be 0 or 1.
//!
//! The names live somewhere else. The LADSPA RDF vocabulary — the one
//! `/usr/share/ladspa/rdf` holds, shipped by swh-plugins, caps, blop and TAP —
//! addresses a port as `&ladspa;<unique id>.<port index>` and hangs a
//! `hasScale` of `Point`s off it, each with a value and a label. That is the
//! same information LV2 puts in `lv2:scalePoint` and CLAP answers by call, and
//! it is where a host is supposed to look for it.
//!
//! # What this does not do
//!
//! It is not an RDF parser. It reads the two shapes those files are actually
//! written in — the port element inline in a plugin's block, and the port
//! element on its own in a "scales" file — and it looks for exactly two
//! attributes. A triple store would be the correct tool and would cost a
//! dependency and a graph query for a lookup table of a few hundred rows.
//!
//! ponytail: no XML dependency, no namespace handling. If a file ever declares
//! the ladspa namespace under another prefix, its scales are missed and the
//! ports stay numbers — which is exactly where they were before.

use std::collections::HashMap;
use std::ffi::c_ulong;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The named positions of one port: `(value, label)`, in value order.
type Points = Vec<(f64, String)>;

/// One port's entry: what the file calls the port, and its named positions.
///
/// The label is kept because it is the only way to tell a good row from a bad
/// one — see [`points_for`].
type Scale = (Option<String>, Points);

/// `(unique id, port index)` → the scale of that port.
///
/// The id is `c_ulong` because that is what a `LADSPA_Descriptor` carries —
/// 64-bit here, 32-bit on an ARM build — so a caller hands its own field over
/// with no cast to get wrong.
type Scales = HashMap<(c_ulong, u32), Scale>;

/// Where the metadata files live, in the order the LADSPA convention names
/// them. `LADSPA_RDF_PATH` wins, as `LADSPA_PATH` does for the plugins.
fn search_path() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("LADSPA_RDF_PATH") {
        return std::env::split_paths(&p).collect();
    }
    let mut out = vec![
        PathBuf::from("/usr/share/ladspa/rdf"),
        PathBuf::from("/usr/local/share/ladspa/rdf"),
        // DSSI plugins are LADSPA plugins with a synth on top, and their
        // metadata is written in the same vocabulary.
        PathBuf::from("/usr/share/dssi/rdf"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(home).join(".ladspa/rdf"));
    }
    out
}

/// The value of `name="..."` (or `name='...'`) in an element's attributes.
fn attr<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let at = head.find(name)?;
    let rest = head[at + name.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &rest[1..];
    let end = body.find(quote)?;
    Some(&body[..end])
}

/// `&ladspa;1675.4` → `(1675, 4)`. Anything else is not a port of ours.
fn port_ref(about: &str) -> Option<(c_ulong, u32)> {
    let tail = about.rsplit(';').next()?;
    let (id, port) = tail.split_once('.')?;
    Some((id.trim().parse().ok()?, port.trim().parse().ok()?))
}

/// Read one file's port scales. Whitespace is collapsed first because caps'
/// file breaks a single element across five lines.
fn parse(text: &str) -> Vec<((c_ulong, u32), Scale)> {
    let flat: String = {
        let mut out = String::with_capacity(text.len());
        let mut space = false;
        for c in text.chars() {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
        out
    };
    let mut out = Vec::new();
    for chunk in flat.split("<ladspa:InputControlPort").skip(1) {
        let Some(head_end) = chunk.find('>') else {
            continue;
        };
        let head = &chunk[..head_end];
        let Some(key) = attr(head, "rdf:about").and_then(port_ref) else {
            continue;
        };
        // `<... />` is a port with no scale hanging off it. Stopping here also
        // keeps the scan from walking into the next plugin's block.
        if head.trim_end().ends_with('/') {
            continue;
        }
        let label = attr(head, "hasLabel").map(str::to_string);
        let body = &chunk[head_end + 1..];
        let body = match body.find("</ladspa:InputControlPort>") {
            Some(end) => &body[..end],
            None => body,
        };
        let mut points: Points = Vec::new();
        for point in body.split("<ladspa:Point").skip(1) {
            let Some(end) = point.find('>') else { continue };
            let head = &point[..end];
            let (Some(value), Some(label)) = (attr(head, "rdf:value"), attr(head, "hasLabel"))
            else {
                continue;
            };
            let Ok(value) = value.trim().parse::<f64>() else {
                continue;
            };
            points.push((value, label.to_string()));
        }
        if points.is_empty() {
            continue;
        }
        // In value order, because that is the order the positions sit in and
        // the order a list of them is stepped through.
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.push((key, (label, points)));
    }
    out
}

/// Every scale on this machine, read once.
///
/// A few hundred kilobytes of XML on the whole search path, parsed the first
/// time a LADSPA plugin is described and then kept: the alternative is
/// re-reading seven files per plugin per scan.
fn scales() -> &'static Scales {
    static SCALES: OnceLock<Scales> = OnceLock::new();
    SCALES.get_or_init(|| {
        let mut out = Scales::new();
        for dir in search_path() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_rdf = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("rdf"));
                if !is_rdf {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (key, scale) in parse(&text) {
                    // First file wins: the search path is in precedence order.
                    out.entry(key).or_insert(scale);
                }
            }
        }
        out
    })
}

/// A name reduced to what two people writing it down would agree on.
fn squashed(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The named positions of port `port` of the plugin with `unique_id`, if the
/// metadata on this machine names them — and if it is talking about the port
/// it thinks it is.
///
/// `port_name` is what the **plugin** calls that port, and it is checked
/// against what the file calls it, because not every file counts its ports the
/// same way: blop's is written one-based, so its "Mode" scale lands on the
/// neighbouring "Steps" port and a step count of a hundred comes back as three
/// names that belong to something else. A wrong name is worse than a number —
/// the number was at least honest — so a row that disagrees is dropped.
///
/// Files that give no label for the port (swh's scales file is written that
/// way) are taken at their word: there is nothing to check against, and swh is
/// where this vocabulary comes from.
pub fn points_for(unique_id: c_ulong, port: u32, port_name: &str) -> Points {
    let Some((label, points)) = scales().get(&(unique_id, port)) else {
        return Points::new();
    };
    if let Some(label) = label {
        let (a, b) = (squashed(label), squashed(port_name));
        if a.is_empty() || !(b.contains(&a) || a.contains(&b)) {
            return Points::new();
        }
    }
    points.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape swh's "scales" file is written in: the port on its own, with
    /// the scale inside it.
    #[test]
    fn a_standalone_port_element_names_its_positions() {
        let text = r#"
<ladspa:InputControlPort rdf:about="&ladspa;1416.0">
  <ladspa:hasScale>
    <ladspa:Scale>
      <ladspa:hasPoint><ladspa:Point rdf:value="2" ladspa:hasLabel="triangle" /></ladspa:hasPoint>
      <ladspa:hasPoint><ladspa:Point rdf:value="1" ladspa:hasLabel="sine" /></ladspa:hasPoint>
    </ladspa:Scale>
  </ladspa:hasScale>
</ladspa:InputControlPort>
"#;
        let got = parse(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, (1416, 0));
        // Sorted by value, whatever order the file put them in.
        assert_eq!(got[0].1 .0, None, "that file names no port");
        assert_eq!(got[0].1 .1[0], (1.0, "sine".to_string()));
        assert_eq!(got[0].1 .1[1], (2.0, "triangle".to_string()));
    }

    /// The shape caps is written in: the port nested in the plugin's block,
    /// attributes split across lines, single quotes, and a sibling port with
    /// no scale that must not inherit one.
    #[test]
    fn a_nested_port_is_read_and_its_neighbour_is_left_alone() {
        let text = r#"
<ladspa:FilterPlugin rdf:about='http://quitte.de/dsp/caps.html#Noisegate'>
  <ladspa:hasPort>
    <ladspa:InputControlPort rdf:about="&ladspa;2601.0"
        ladspa:hasLabel="mode">
      <ladspa:hasScale><ladspa:Scale>
        <ladspa:hasPoint><ladspa:Point
            rdf:value="0"
            ladspa:hasLabel="off" />
        </ladspa:hasPoint>
        <ladspa:hasPoint><ladspa:Point
            rdf:value="50"
            ladspa:hasLabel="global" />
        </ladspa:hasPoint>
      </ladspa:Scale></ladspa:hasScale>
    </ladspa:InputControlPort>
  </ladspa:hasPort>
  <ladspa:hasPort>
    <ladspa:InputControlPort rdf:about="&ladspa;2601.1" ladspa:hasLabel="threshold" />
  </ladspa:hasPort>
</ladspa:FilterPlugin>
"#;
        let got = parse(text);
        assert_eq!(got.len(), 1, "only the port that has a scale");
        assert_eq!(got[0].0, (2601, 0));
        assert_eq!(got[0].1 .0.as_deref(), Some("mode"));
        assert_eq!(
            got[0].1 .1,
            vec![(0.0, "off".to_string()), (50.0, "global".to_string())]
        );
    }

    /// A file that says nothing about scales says nothing at all — the ports
    /// stay numbers, which is where they were.
    #[test]
    fn a_file_with_no_scales_yields_none() {
        assert!(parse("<rdf:RDF></rdf:RDF>").is_empty());
        assert!(parse("").is_empty());
    }

    /// Whatever is installed here, reading the whole search path must not
    /// panic and must not invent a port.
    #[test]
    fn the_machines_own_metadata_reads_without_a_fuss() {
        let all = scales();
        for ((id, port), (_, points)) in all.iter() {
            assert!(*id > 0, "a plugin id is never zero");
            assert!(*port < 1024, "port {port} of {id} is out of any range");
            assert!(!points.is_empty(), "an empty scale is not stored");
        }
    }

    /// A file whose port index is off by one must not hand its names to the
    /// neighbouring port. blop's is written that way: its `Mode` scale is
    /// addressed to the port the plugin calls `Steps (1 - 100)`.
    #[test]
    fn a_scale_that_names_another_port_is_not_taken() {
        assert_eq!(squashed("mains (Hz)"), "mainshz");
        // The check itself, without touching the machine's own files.
        let agree = |rdf: &str, plugin: &str| {
            let (a, b) = (squashed(rdf), squashed(plugin));
            !a.is_empty() && (b.contains(&a) || a.contains(&b))
        };
        assert!(agree("mains (Hz)", "mains (Hz)"));
        assert!(agree("Mode", "Mode (0 = Extend, 1 = Wrap, 2 = Clip)"));
        assert!(!agree("Mode", "Steps (1 - 100)"));
    }
}
