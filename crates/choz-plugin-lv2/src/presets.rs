//! The presets an LV2 bundle publishes, through `pset:Preset`.
//!
//! LV2 keeps its presets in the same Turtle the rest of the bundle is described
//! in: a subject typed `pset:Preset` that `lv2:appliesTo` the plugin, with an
//! `rdfs:label`, an optional `pset:bank`, and either one `lv2:port [ lv2:symbol
//! … ; pset:value … ]` per control it sets or a `state:state` blob for the
//! plugins whose patch is not a set of control ports (ZynAddSubFX's 2000
//! instruments are one `urn:distrho:state` document each).
//!
//! Two things about where they live:
//!
//! * **They are not always in the plugin's own bundle.** ZynAddSubFX ships
//!   `ZynAddSubFX.lv2` and, beside it, `ZynAddSubFX.lv2presets` — a second
//!   bundle holding nothing but banks and presets that `lv2:appliesTo` the
//!   first. Any sibling directory whose name starts with the plugin bundle's is
//!   read too, which is what makes those banks show up at all.
//! * **Only the manifests are read up front.** A preset's contents sit in the
//!   `rdfs:seeAlso` document, and Zyn's are 35 MB of Turtle across forty files.
//!   The list needs only the names and banks, so the document is parsed when a
//!   preset is actually applied.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use choz_ports::PresetEntry;

use crate::discovery::Port;
use crate::editor::SharedControls;
use crate::state::SharedState;
use crate::ttl::{self, Graph, Node};

/// `pset:Preset`, and the predicates that describe one.
const PSET_PRESET: &str = "http://lv2plug.in/ns/ext/presets#Preset";
const PSET_VALUE: &str = "http://lv2plug.in/ns/ext/presets#value";
const PSET_BANK: &str = "http://lv2plug.in/ns/ext/presets#bank";
/// `state:state`, the other half of what a preset can carry.
const STATE_STATE: &str = "http://lv2plug.in/ns/ext/state#state";
/// What a Turtle string literal is, when the plugin is handed one back.
const ATOM_STRING: &str = "http://lv2plug.in/ns/ext/atom#String";

/// One preset, as the manifest describes it. What it *does* is read from
/// [`Self::doc`] when it is applied.
#[derive(Clone)]
pub struct Lv2Preset {
    pub entry: PresetEntry,
    /// The documents its contents are in: the manifest that named it, plus the
    /// `rdfs:seeAlso` it points at.
    doc: Vec<PathBuf>,
}

/// Every preset that applies to `plugin_uri`, from `bundle_dir` and from any
/// sibling bundle that extends it.
///
/// Reads and parses Turtle: UI thread only.
pub fn scan(bundle_dir: &Path, plugin_uri: &str, _ports: &[Port]) -> Vec<Lv2Preset> {
    let mut out: Vec<Lv2Preset> = Vec::new();
    for manifest in manifests(bundle_dir) {
        out.extend(scan_manifest(&manifest, plugin_uri));
    }
    out.sort_by(|a, b| (&a.entry.category, &a.entry.name).cmp(&(&b.entry.category, &b.entry.name)));
    out
}

/// The manifests to read: the bundle's own, and those of sibling directories
/// whose name starts with it — `ZynAddSubFX.lv2` and `ZynAddSubFX.lv2presets`.
fn manifests(bundle_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![bundle_dir.join("manifest.ttl")];
    let (Some(parent), Some(name)) = (bundle_dir.parent(), bundle_dir.file_name()) else {
        return out;
    };
    let Some(name) = name.to_str() else {
        return out;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path == bundle_dir || !path.is_dir() {
            continue;
        }
        let extends = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(name));
        if extends && path.join("manifest.ttl").is_file() {
            out.push(path.join("manifest.ttl"));
        }
    }
    out
}

/// The presets one manifest declares for `plugin_uri`.
fn scan_manifest(manifest_path: &Path, plugin_uri: &str) -> Vec<Lv2Preset> {
    let Ok(graph) = Graph::parse_file(manifest_path) else {
        return Vec::new();
    };
    graph
        .subjects_of_type(PSET_PRESET)
        .into_iter()
        .filter(|uri| {
            graph
                .objects(uri, ttl::LV2_APPLIES_TO)
                .iter()
                .any(|o| o.as_str() == plugin_uri)
        })
        .map(|uri| {
            let name = graph
                .object(&uri, ttl::RDFS_LABEL)
                .map(|n| n.as_str().to_string())
                .filter(|l| !l.is_empty())
                // A preset with no label is still selectable: its URI ends in
                // something the user can recognise more often than not.
                .unwrap_or_else(|| uri.rsplit(['#', '/']).next().unwrap_or(&uri).to_string());
            let category = graph
                .object(&uri, PSET_BANK)
                .and_then(|bank| graph.object(bank.as_str(), ttl::RDFS_LABEL))
                .map(|n| n.as_str().to_string())
                .unwrap_or_default();
            // Zyn labels every preset "Bank: 0001-Name.xiz"; the bank is
            // already the sidebar, so it is not the name as well.
            let name = match name.split_once(": ") {
                Some((bank, rest)) if !category.is_empty() && bank == category => rest.to_string(),
                _ => name,
            };
            let mut doc = vec![manifest_path.to_path_buf()];
            for see in graph.objects(&uri, ttl::RDFS_SEE_ALSO) {
                let Node::Iri(iri) = see else { continue };
                if let Some(path) = ttl::file_uri_to_path(iri).filter(|p| !doc.contains(p)) {
                    doc.push(path);
                }
            }
            Lv2Preset {
                entry: PresetEntry {
                    name,
                    category,
                    key: uri,
                },
                doc,
            }
        })
        .collect()
}

/// What applying a preset does: port values, opaque state, or both.
#[derive(Default)]
struct Contents {
    values: Vec<(usize, f32)>,
    /// `(key URI, type URI, bytes)` for `state:state`.
    state: Vec<(String, String, Vec<u8>)>,
}

/// Read one preset's contents out of the documents that hold it.
fn contents(preset: &Lv2Preset, by_symbol: &HashMap<String, usize>) -> Contents {
    let mut graph = Graph::default();
    for path in &preset.doc {
        if let Ok(g) = Graph::parse_file(path) {
            graph.extend_from(&g);
        }
    }
    graph.index();
    let uri = &preset.entry.key;
    let values = graph
        .objects(uri, ttl::LV2_PORT)
        .into_iter()
        .filter_map(|port| {
            let symbol = graph.object(port.as_str(), ttl::LV2_SYMBOL)?;
            let value = graph.object(port.as_str(), PSET_VALUE)?;
            let index = *by_symbol.get(symbol.as_str())?;
            Some((index, value.as_str().parse::<f32>().ok()?))
        })
        .collect();
    // `state:state [ <urn:distrho:state> "…" ]`: one blank node whose every
    // triple is a property of the state.
    let state = match graph.object(uri, STATE_STATE) {
        Some(node) => {
            let subject = node.as_str().to_string();
            graph
                .triples
                .iter()
                .filter(|t| t.s.as_str() == subject && t.p != ttl::RDF_TYPE)
                .map(|t| {
                    // NUL-terminated: `atom:String` is a C string, and a plugin
                    // that reads it with `strlen` walks off the end without it.
                    let mut bytes = t.o.as_str().as_bytes().to_vec();
                    bytes.push(0);
                    (t.p.clone(), ATOM_STRING.to_string(), bytes)
                })
                .collect()
        }
        None => Vec::new(),
    };
    Contents { values, state }
}

/// The scanned presets, the control buffer to write them into, and the instance
/// to hand a `state:state` one to.
pub struct Lv2Presets {
    controls: SharedControls,
    state: SharedState,
    presets: Vec<Lv2Preset>,
    by_symbol: HashMap<String, usize>,
    /// The last preset read, kept because a 1.5 MB bank document is parsed
    /// again for every arrow press through it otherwise.
    ///
    /// ponytail: one entry is enough for walking a bank; a real cache if
    /// anything ever needs two.
    last: Mutex<Option<(String, Contents)>>,
}

impl Lv2Presets {
    pub fn new(
        controls: SharedControls,
        state: SharedState,
        presets: Vec<Lv2Preset>,
        ports: &[Port],
    ) -> Self {
        Self {
            controls,
            state,
            presets,
            by_symbol: ports
                .iter()
                .map(|p| (p.symbol.clone(), p.index as usize))
                .collect(),
            last: Mutex::new(None),
        }
    }
}

impl choz_ports::PluginPresets for Lv2Presets {
    fn list(&self) -> Vec<PresetEntry> {
        self.presets.iter().map(|p| p.entry.clone()).collect()
    }

    fn load(&self, key: &str) {
        let Some(preset) = self.presets.iter().find(|p| p.entry.key == key) else {
            return;
        };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref().map(|(k, _)| k.as_str()) != Some(key) {
            *last = Some((key.to_string(), contents(preset, &self.by_symbol)));
        }
        let Some((_, Contents { values, state })) = last.as_ref() else {
            return;
        };

        if !values.is_empty() {
            let guard = self.controls.lock();
            if let Some(cell) = guard.as_ref() {
                for &(index, value) in values {
                    if index >= cell.len {
                        continue;
                    }
                    // SAFETY: the cell's buffer is the instance's control array,
                    // alive for as long as the cell is `Some`, and `index` is
                    // inside it. This is the same store the plugin's own window
                    // makes — "latest value wins" is the whole of the LV2
                    // control-port protocol.
                    unsafe { cell.values.add(index).write(value) };
                }
            }
        }
        if !state.is_empty() {
            crate::state::restore_state(&self.state, state);
        }
    }
}
