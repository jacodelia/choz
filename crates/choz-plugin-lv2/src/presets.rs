//! The presets an LV2 bundle publishes, through `pset:Preset`.
//!
//! LV2 keeps its presets in the same Turtle the rest of the bundle is described
//! in: a subject typed `pset:Preset` that `lv2:appliesTo` the plugin, with an
//! `rdfs:label` and one `lv2:port [ lv2:symbol … ; pset:value … ]` per control
//! it sets. There is nothing to ask the plugin — which is why the whole list is
//! resolved to port indices at load time and applying one is a handful of
//! stores into the control buffer the plugin already reads from.
//!
//! **What this does not do**: `state:state` properties. A preset may also carry
//! opaque state (a sample path, a wavetable) through the state extension, and
//! those presets arrive here with only their control-port half applied. The
//! bundles that ship presets on this machine (mda's 230, the Zyn effects,
//! fat1) are all pure control-port ones.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use choz_ports::PresetEntry;

use crate::discovery::Port;
use crate::editor::SharedControls;
use crate::ttl::{self, Graph, Node};

/// `pset:Preset`, and the two predicates that describe one.
const PSET_PRESET: &str = "http://lv2plug.in/ns/ext/presets#Preset";
const PSET_VALUE: &str = "http://lv2plug.in/ns/ext/presets#value";
const PSET_BANK: &str = "http://lv2plug.in/ns/ext/presets#bank";

/// One preset, resolved to what applying it actually does.
#[derive(Clone)]
pub struct Lv2Preset {
    pub entry: PresetEntry,
    /// `(control port index, value)`, already matched to this plugin's ports.
    values: Vec<(usize, f32)>,
}

/// Every preset in `bundle_dir` that applies to `plugin_uri`, resolved against
/// `ports` so nothing has to be looked up again when one is picked.
///
/// Reads and parses Turtle: UI thread only.
pub fn scan(bundle_dir: &Path, plugin_uri: &str, ports: &[Port]) -> Vec<Lv2Preset> {
    let Ok(manifest) = Graph::parse_file(&bundle_dir.join("manifest.ttl")) else {
        return Vec::new();
    };

    // The manifest names the presets but usually keeps their contents in
    // another file — one `rdfs:seeAlso` document per plugin, shared by all of
    // its presets. Parse each one once.
    let ours: Vec<String> = manifest
        .subjects_of_type(PSET_PRESET)
        .into_iter()
        .filter(|uri| {
            manifest
                .objects(uri, ttl::LV2_APPLIES_TO)
                .iter()
                .any(|o| o.as_str() == plugin_uri)
        })
        .collect();
    if ours.is_empty() {
        return Vec::new();
    }

    let mut graph = Graph::default();
    graph.extend_from(&manifest);
    let mut seen: Vec<PathBuf> = Vec::new();
    for uri in &ours {
        for see in manifest.objects(uri, ttl::RDFS_SEE_ALSO) {
            let Node::Iri(iri) = see else { continue };
            let Some(path) = ttl::file_uri_to_path(iri) else {
                continue;
            };
            if !seen.contains(&path) {
                seen.push(path.clone());
                if let Ok(g) = Graph::parse_file(&path) {
                    graph.extend_from(&g);
                }
            }
        }
    }
    graph.index();

    let by_symbol: HashMap<&str, usize> = ports
        .iter()
        .map(|p| (p.symbol.as_str(), p.index as usize))
        .collect();

    let mut out: Vec<Lv2Preset> = ours
        .into_iter()
        .map(|uri| {
            let name = graph
                .object(&uri, ttl::RDFS_LABEL)
                .map(|n| n.as_str().to_string())
                // A preset with no label is still selectable: its URI ends in
                // something the user can recognise more often than not.
                .unwrap_or_else(|| uri.rsplit(['#', '/']).next().unwrap_or(&uri).to_string());
            let category = graph
                .object(&uri, PSET_BANK)
                .and_then(|bank| graph.object(bank.as_str(), ttl::RDFS_LABEL))
                .map(|n| n.as_str().to_string())
                .unwrap_or_default();
            let values = graph
                .objects(&uri, ttl::LV2_PORT)
                .into_iter()
                .filter_map(|port| {
                    let symbol = graph.object(port.as_str(), ttl::LV2_SYMBOL)?;
                    let value = graph.object(port.as_str(), PSET_VALUE)?;
                    let index = *by_symbol.get(symbol.as_str())?;
                    Some((index, value.as_str().parse::<f32>().ok()?))
                })
                .collect();
            Lv2Preset {
                entry: PresetEntry {
                    name,
                    category,
                    key: uri,
                },
                values,
            }
        })
        // A preset that sets no port of this plugin sets nothing: showing it
        // would be a row that does nothing when picked.
        .filter(|p| !p.values.is_empty())
        .collect();
    out.sort_by(|a, b| (&a.entry.category, &a.entry.name).cmp(&(&b.entry.category, &b.entry.name)));
    out
}

/// The scanned presets plus the control buffer to write them into.
pub struct Lv2Presets {
    controls: SharedControls,
    presets: Vec<Lv2Preset>,
}

impl Lv2Presets {
    pub fn new(controls: SharedControls, presets: Vec<Lv2Preset>) -> Self {
        Self { controls, presets }
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
        let guard = self.controls.lock();
        let Some(cell) = guard.as_ref() else { return };
        for &(index, value) in &preset.values {
            if index >= cell.len {
                continue;
            }
            // SAFETY: the cell's buffer is the instance's control array, alive
            // for as long as the cell is `Some`, and `index` is inside it. This
            // is the same store the plugin's own window makes — "latest value
            // wins" is the whole of the LV2 control-port protocol.
            unsafe { cell.values.add(index).write(value) };
        }
    }
}
