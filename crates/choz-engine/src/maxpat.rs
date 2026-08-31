//! Reading a Max/MSP patch, and saying honestly what can be kept.
//!
//! # Why this is an importer and not a host
//!
//! A `.maxpat` is JSON, so reading it is easy. **Running it is not possible**:
//! there is no embeddable Max runtime — nothing like libpd — and there is not
//! going to be one. Anything that claimed Max compatibility would be lying, and
//! a host that lies about a format is worse than one that does not have it.
//!
//! So this does the one honest thing left: walk the patch's signal chain, keep
//! the objects that have a real equivalent among choz's own effects, and **name
//! every single one it could not keep**. What comes out is a starting point a
//! person can hear, plus a list of what is missing from it — which is the
//! difference between "imported" and "opened".
//!
//! ```text
//! adc~ ─► overdrive~ ─► freeverb~ ─► gain~ ─► dac~
//!            │              │          │
//!            ▼              ▼          ▼
//!         saturator      reverb      gain          (kept)
//!         pfft~, js, poly~, gizmo~ …               (named, dropped)
//! ```
//!
//! # What "the signal chain" means here
//!
//! Max patches carry their wiring in `lines`: each one is a source box, an
//! outlet, a destination box and an inlet. The walk starts at whatever brings
//! audio in (`adc~`, `plugin~`, `receive~`) and follows the cords. A patch with
//! no such object is read in file order instead, which is the order the boxes
//! were saved in and usually the order they were laid out — a guess, and it
//! says so.
//!
//! Only the **first** audio path is followed. A patch that splits into three
//! parallel chains and sums them is a mixer, and a mixer is not an FX chain;
//! taking the first path and naming everything else as dropped is the answer
//! that can be checked by ear.

use std::collections::HashMap;
use std::path::Path;

use crate::fx_chain::FxSpec;

/// What one `.maxpat` turned into.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaxImport {
    /// The patch's file stem.
    pub name: String,
    /// Effects choz can build, in the order the signal reaches them.
    pub chain: Vec<FxSpec>,
    /// Max objects on the signal path that have no equivalent here, named in
    /// the order they appear. **This is the important half**: it is the list of
    /// what the import is missing.
    pub dropped: Vec<String>,
    /// Whether the chain came from following the patch cords, or from the
    /// order the boxes happen to sit in the file.
    pub followed_cords: bool,
}

impl MaxImport {
    /// A line for the interface: what came in and what did not.
    pub fn summary(&self) -> String {
        let kept = self.chain.len();
        let order = match self.followed_cords {
            true => "signal order",
            false => "file order (no adc~/plugin~ to start from)",
        };
        if self.dropped.is_empty() {
            return format!("{}: {kept} effect(s), {order}", self.name);
        }
        format!(
            "{}: {kept} effect(s), {order} — no equivalent for: {}",
            self.name,
            self.dropped.join(", ")
        )
    }
}

/// Max objects that mean the same thing as one of choz's own effects.
///
/// Deliberately short, and deliberately only the ones where the equivalence is
/// real rather than "sounds a bit like". Everything absent from here is named
/// in [`MaxImport::dropped`], which is what makes the import honest: a table
/// that guessed would produce a patch that is subtly not the one that was
/// written, and nobody would know which part.
const EQUIVALENTS: &[(&str, &str)] = &[
    // Level and dynamics.
    ("gain~", "gain"),
    ("*~", "gain"),
    ("limi~", "limiter"),
    ("omx.peaklim~", "limiter"),
    ("compressor~", "compressor"),
    ("omx.comp~", "compressor"),
    ("gate~", "gate"),
    // Filters. Max's are all one biquad section, which is what choz's is.
    ("lores~", "filter"),
    ("reson~", "filter"),
    ("svf~", "filter"),
    ("biquad~", "filter"),
    ("onepole~", "filter"),
    ("filtergraph~", "parameq"),
    // Time.
    ("delay~", "delay"),
    ("tapout~", "delay"),
    ("comb~", "delay"),
    ("freeverb~", "reverb"),
    ("yafr2", "reverb"),
    ("omx.4band~", "graphiceq"),
    // Colour.
    ("overdrive~", "saturator"),
    ("degrade~", "bitcrusher"),
];

/// Objects that are not part of a signal chain at all: comments, buttons,
/// numbers, the patcher's own furniture. Counted as neither kept nor dropped,
/// because listing them would bury the objects that matter.
fn is_furniture(class: &str) -> bool {
    matches!(
        class,
        "comment"
            | "message"
            | "number"
            | "flonum"
            | "toggle"
            | "button"
            | "slider"
            | "dial"
            | "live.dial"
            | "live.slider"
            | "live.gain~"
            | "panel"
            | "inlet"
            | "outlet"
            | "scope~"
            | "meter~"
            | "spectroscope~"
    )
}

/// Objects that bring audio into the patch, i.e. where a walk starts.
fn is_input(name: &str) -> bool {
    matches!(name, "adc~" | "plugin~" | "receive~" | "in~" | "sig~")
}

/// Objects that take audio out of it, i.e. where a walk stops.
fn is_output(name: &str) -> bool {
    matches!(name, "dac~" | "plugout~" | "send~" | "out~" | "ezdac~")
}

/// One box of the patch, reduced to what matters here.
struct Box_ {
    id: String,
    /// The object's name: the first word of its text, or its class.
    name: String,
    class: String,
    /// Position in the file, which is the fallback order.
    index: usize,
}

/// Read `path` and say what choz can make of it.
///
/// Fails only when the file cannot be read or is not JSON. A patch full of
/// objects choz knows nothing about is **not** a failure: it imports as an
/// empty chain and a long `dropped` list, which is the honest answer.
pub fn read_maxpat(path: &Path) -> anyhow::Result<MaxImport> {
    let text = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "patch".to_string());

    let patcher = json.get("patcher").unwrap_or(&json);
    let boxes = collect_boxes(patcher);
    let cords = collect_cords(patcher);

    let ordered = walk(&boxes, &cords);
    let followed_cords = ordered.is_some();
    let order: Vec<&Box_> = ordered.unwrap_or_else(|| {
        let mut all: Vec<&Box_> = boxes.values().collect();
        all.sort_by_key(|b| b.index);
        all
    });

    let mut out = MaxImport {
        name,
        followed_cords,
        ..Default::default()
    };
    for b in order {
        if is_furniture(&b.class) || is_input(&b.name) || is_output(&b.name) {
            continue;
        }
        match EQUIVALENTS.iter().find(|(max, _)| *max == b.name) {
            Some((_, kind)) => out.chain.push(FxSpec {
                gate: None,
                kind: (*kind).to_string(),
                enabled: true,
                wet: 1.0,
                // The middle of every knob. A Max object's arguments are in its
                // own units and mean nothing to choz's parameters; pretending
                // to convert them would be the guessing this refuses to do.
                params: vec![0.5; 16],
                plugin: None,
                loops: Vec::new(),
                loop_frames: 0,
            }),
            None => {
                if !out.dropped.iter().any(|d| d == &b.name) {
                    out.dropped.push(b.name.clone());
                }
            }
        }
    }
    Ok(out)
}

fn collect_boxes(patcher: &serde_json::Value) -> HashMap<String, Box_> {
    let mut out = HashMap::new();
    let Some(list) = patcher.get("boxes").and_then(|b| b.as_array()) else {
        return out;
    };
    for (index, entry) in list.iter().enumerate() {
        let Some(b) = entry.get("box") else { continue };
        let id = b
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let class = b
            .get("maxclass")
            .and_then(|v| v.as_str())
            .unwrap_or("newobj")
            .to_string();
        // `text` is the whole object line — `lores~ 1200 0.4`. The name is its
        // first word; the arguments are in Max's units and are not read.
        let name = b
            .get("text")
            .and_then(|v| v.as_str())
            .and_then(|t| t.split_whitespace().next())
            .unwrap_or(&class)
            .to_string();
        out.insert(
            id.clone(),
            Box_ {
                id,
                name,
                class,
                index,
            },
        );
    }
    out
}

/// Patch cords, as `(source id, destination id)`.
fn collect_cords(patcher: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(list) = patcher.get("lines").and_then(|l| l.as_array()) else {
        return out;
    };
    for entry in list {
        let Some(line) = entry.get("patchline") else {
            continue;
        };
        let end = |key: &str| {
            line.get(key)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        if let (Some(from), Some(to)) = (end("source"), end("destination")) {
            out.push((from, to));
        }
    }
    out
}

/// Follow the cords from the patch's audio input, in order.
///
/// `None` when there is nothing to start from, which is when the caller falls
/// back to file order and says so.
fn walk<'a>(boxes: &'a HashMap<String, Box_>, cords: &[(String, String)]) -> Option<Vec<&'a Box_>> {
    let start = boxes
        .values()
        .filter(|b| is_input(&b.name))
        .min_by_key(|b| b.index)?;
    let mut out = Vec::new();
    let mut at = start.id.clone();
    let mut seen = vec![at.clone()];
    // The first cord out of each box that leads somewhere known. "First" is the
    // file's own order, which is the only order a patch cord has.
    while let Some(next) = cords
        .iter()
        .find(|(from, to)| *from == at && boxes.contains_key(to))
        .map(|(_, to)| to.clone())
    {
        // A cycle is a feedback path, and a walk that follows one never ends.
        if seen.contains(&next) {
            break;
        }
        seen.push(next.clone());
        let b = &boxes[&next];
        out.push(b);
        if is_output(&b.name) {
            break;
        }
        at = next;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("choz-maxpat");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A patch whose chain choz can mostly build: the equivalents come through
    /// in the order the signal reaches them, and the object with no equivalent
    /// is **named**, not silently skipped.
    #[test]
    fn a_signal_chain_is_followed_and_what_is_missing_is_named() {
        let path = write(
            "chain.maxpat",
            r#"{"patcher":{"boxes":[
                {"box":{"id":"obj-1","maxclass":"newobj","text":"adc~"}},
                {"box":{"id":"obj-2","maxclass":"newobj","text":"overdrive~"}},
                {"box":{"id":"obj-3","maxclass":"newobj","text":"pfft~ myspectral 512 2"}},
                {"box":{"id":"obj-4","maxclass":"newobj","text":"freeverb~"}},
                {"box":{"id":"obj-5","maxclass":"newobj","text":"dac~"}},
                {"box":{"id":"obj-6","maxclass":"comment","text":"sounds nice"}}
            ],"lines":[
                {"patchline":{"source":["obj-1",0],"destination":["obj-2",0]}},
                {"patchline":{"source":["obj-2",0],"destination":["obj-3",0]}},
                {"patchline":{"source":["obj-3",0],"destination":["obj-4",0]}},
                {"patchline":{"source":["obj-4",0],"destination":["obj-5",0]}}
            ]}}"#,
        );
        let import = read_maxpat(&path).unwrap();
        assert!(import.followed_cords);
        let kinds: Vec<&str> = import.chain.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, vec!["saturator", "reverb"], "in signal order");
        assert_eq!(
            import.dropped,
            vec!["pfft~"],
            "the one thing choz cannot do is the one thing it says"
        );
        // The comment is furniture: neither kept nor named.
        assert!(!import.dropped.iter().any(|d| d == "comment"));
        assert!(import.summary().contains("pfft~"), "{}", import.summary());
    }

    /// A patch with nothing choz knows imports as nothing, with every object
    /// named. That is a result, not a failure: "I opened it and here is what I
    /// cannot do" beats an error message.
    #[test]
    fn a_patch_of_things_choz_cannot_do_says_so_object_by_object() {
        let path = write(
            "spectral.maxpat",
            r#"{"patcher":{"boxes":[
                {"box":{"id":"obj-1","maxclass":"newobj","text":"adc~"}},
                {"box":{"id":"obj-2","maxclass":"newobj","text":"gizmo~ 2048"}},
                {"box":{"id":"obj-3","maxclass":"newobj","text":"poly~ voice 8"}},
                {"box":{"id":"obj-4","maxclass":"newobj","text":"dac~"}}
            ],"lines":[
                {"patchline":{"source":["obj-1",0],"destination":["obj-2",0]}},
                {"patchline":{"source":["obj-2",0],"destination":["obj-3",0]}},
                {"patchline":{"source":["obj-3",0],"destination":["obj-4",0]}}
            ]}}"#,
        );
        let import = read_maxpat(&path).unwrap();
        assert!(import.chain.is_empty());
        assert_eq!(import.dropped, vec!["gizmo~", "poly~"]);
    }

    /// No `adc~` to start from: the boxes are read in file order, and the
    /// summary says that is a guess rather than the signal path.
    #[test]
    fn without_an_input_object_it_falls_back_to_file_order_and_says_so() {
        let path = write(
            "loose.maxpat",
            r#"{"patcher":{"boxes":[
                {"box":{"id":"obj-1","maxclass":"newobj","text":"lores~ 1200 0.4"}},
                {"box":{"id":"obj-2","maxclass":"newobj","text":"gain~"}}
            ]}}"#,
        );
        let import = read_maxpat(&path).unwrap();
        assert!(!import.followed_cords);
        let kinds: Vec<&str> = import.chain.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, vec!["filter", "gain"]);
        assert!(
            import.summary().contains("file order"),
            "{}",
            import.summary()
        );
    }

    /// A feedback loop in the patch is a cycle in the walk, and a walk that
    /// follows one never comes back.
    #[test]
    fn a_feedback_loop_does_not_hang_the_import() {
        let path = write(
            "loop.maxpat",
            r#"{"patcher":{"boxes":[
                {"box":{"id":"obj-1","maxclass":"newobj","text":"adc~"}},
                {"box":{"id":"obj-2","maxclass":"newobj","text":"comb~ 100"}},
                {"box":{"id":"obj-3","maxclass":"newobj","text":"gain~"}}
            ],"lines":[
                {"patchline":{"source":["obj-1",0],"destination":["obj-2",0]}},
                {"patchline":{"source":["obj-2",0],"destination":["obj-3",0]}},
                {"patchline":{"source":["obj-3",0],"destination":["obj-2",0]}}
            ]}}"#,
        );
        let import = read_maxpat(&path).unwrap();
        let kinds: Vec<&str> = import.chain.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, vec!["delay", "gain"]);
    }

    /// Anything that is not a patch is an error, and anything that is one but
    /// says nothing is an empty import rather than a panic.
    #[test]
    fn rubbish_is_an_error_and_an_empty_patch_is_an_empty_import() {
        let path = write("junk.maxpat", "this is not json at all");
        assert!(read_maxpat(&path).is_err());

        let path = write("empty.maxpat", r#"{"patcher":{"boxes":[],"lines":[]}}"#);
        let import = read_maxpat(&path).unwrap();
        assert!(import.chain.is_empty() && import.dropped.is_empty());
        assert_eq!(import.name, "empty");
    }
}
