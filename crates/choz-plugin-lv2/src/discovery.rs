//! Turn LV2 bundle TTL into structured plugin + port metadata.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

macro_rules! debug { ($($t:tt)*) => {{ if false { eprintln!($($t)*); } }}; }
macro_rules! warn { ($($t:tt)*) => {{ eprintln!("choz-lv2: {}", format_args!($($t)*)); }}; }

use crate::ttl::{self, Graph, Node};

/// What a port carries and which direction it flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    AudioInput,
    AudioOutput,
    ControlInput,
    ControlOutput,
    AtomInput,
    AtomOutput,
    Unknown,
}

impl PortKind {
    pub fn is_audio(self) -> bool {
        matches!(self, PortKind::AudioInput | PortKind::AudioOutput)
    }
    pub fn is_control(self) -> bool {
        matches!(self, PortKind::ControlInput | PortKind::ControlOutput)
    }
    pub fn is_input(self) -> bool {
        matches!(
            self,
            PortKind::AudioInput | PortKind::ControlInput | PortKind::AtomInput
        )
    }
}

/// A single plugin port parsed from the TTL.
#[derive(Debug, Clone)]
pub struct Port {
    pub index: u32,
    pub symbol: String,
    pub name: String,
    pub kind: PortKind,
    /// True for an atom input port that accepts `midi:MidiEvent`.
    pub is_midi: bool,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

/// An X11 editor shipped in the same bundle as the plugin.
///
/// Only `ui:X11UI` is looked for: it is the one that embeds into a plain X11
/// window, which is what choz's editor thread hands the plugin. Gtk/Qt UIs need
/// a toolkit host loop (that is what suil is for) and are ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct Lv2UiInfo {
    pub uri: String,
    /// The UI's own shared object — a *different* binary from the plugin's.
    pub binary_path: PathBuf,
}

/// Host features choz passes to a UI. A UI that requires anything else is not
/// offered an editor at all: instantiating it without what it asked for is how
/// guitarix's UI segfaulted the probe rather than politely returning null.
///
/// `instance-access` / `data-access` are the notable absentees — they hand the
/// UI a pointer to the live plugin instance, which in choz lives on the audio
/// thread and is exactly what must not be shared.
pub const SUPPORTED_UI_FEATURES: &[&str] = &[
    "http://lv2plug.in/ns/extensions/ui#parent",
    "http://lv2plug.in/ns/extensions/ui#idleInterface",
    "http://lv2plug.in/ns/extensions/ui#noUserResize",
    "http://lv2plug.in/ns/ext/urid#map",
    "http://lv2plug.in/ns/ext/options#options",
];

/// Plugin families whose X11 UI segfaults the host on `instantiate`.
///
/// Measured by opening and closing every installed X11 UI in turn
/// (`examples/ui_probe`): every single guitarix UI crashed — with the parent
/// window both mapped and unmapped, with `ui:idleInterface` and `opts:options`
/// supplied, and on its own in a fresh process. Its UI links cairo but not
/// libX11, so it is not the plain X11 embed it declares.
///
/// LSP is **not** listed even though the probe crashes on a few of its UIs each
/// run: the ones that crash differ every time, so it is not a property of any
/// one plugin. That sweep opens 250+ UIs in a single process without unloading
/// any, which is not how choz uses them (one window at a time). Blaming
/// individual URIs there would have been a guess dressed up as a measurement.
///
/// ponytail: a name list is the same blunt instrument the Carla deny-list is,
/// and for the same reason — the crash happens *inside* the plugin, so there is
/// nothing to check for beforehand. The real fix is running editors in the
/// sandbox that already exists for DSP; see "Pendiente" in docs/roadmap.md.
const UI_DENY_PREFIXES: &[&str] = &["http://guitarix.sourceforge.net/plugins/"];

/// A discovered LV2 plugin: identity, binary, classification, and ports.
#[derive(Debug, Clone)]
pub struct Lv2PluginInfo {
    pub uri: String,
    pub name: String,
    pub bundle_dir: PathBuf,
    pub binary_path: PathBuf,
    pub is_instrument: bool,
    pub is_effect: bool,
    pub ports: Vec<Port>,
    /// Required-feature URIs declared by the plugin (we support only urid:map/unmap).
    pub required_features: Vec<String>,
    /// The bundle's X11 editor, when it ships one.
    pub x11_ui: Option<Lv2UiInfo>,
}

/// Parse every plugin in `bundle_dir` (a `*.lv2` directory). Returns an empty
/// vec on any error (logged), so a bad bundle never aborts a scan.
pub fn discover_bundle(bundle_dir: &Path) -> Vec<Lv2PluginInfo> {
    let manifest = bundle_dir.join("manifest.ttl");
    let mgraph = match Graph::parse_file(&manifest) {
        Ok(g) => g,
        Err(e) => {
            debug!("LV2: no manifest in {}: {e}", bundle_dir.display());
            return Vec::new();
        }
    };

    let plugin_uris = mgraph.subjects_of_type(ttl::LV2_PLUGIN);
    if plugin_uris.is_empty() {
        return Vec::new();
    }

    // Build ONE combined graph for the whole bundle: the manifest plus every
    // distinct `rdfs:seeAlso` document, each parsed exactly once. (Multiple
    // plugins in a bundle typically share one big description `.ttl`.)
    let mut graph = Graph::default();
    graph.extend_from(&mgraph);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for uri in &plugin_uris {
        for see in mgraph.objects(uri, ttl::RDFS_SEE_ALSO) {
            let Some(p) = node_to_path(see) else { continue };
            if seen.insert(p.clone())
                && let Ok(g) = Graph::parse_file(&p)
            {
                graph.extend_from(&g);
            }
        }
    }
    graph.index();

    let mut out = Vec::new();
    for uri in plugin_uris {
        let binary = graph.object(&uri, ttl::LV2_BINARY).and_then(node_to_path);
        let Some(binary_path) = binary else {
            warn!("LV2: plugin {uri} has no lv2:binary; skipping");
            continue;
        };
        let info = parse_plugin(&graph, &uri, bundle_dir, binary_path);
        out.push(info);
    }
    out
}

fn parse_plugin(graph: &Graph, uri: &str, bundle_dir: &Path, binary_path: PathBuf) -> Lv2PluginInfo {
    let name = graph
        .object(uri, ttl::DOAP_NAME)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| {
            // Fall back to the last URI path segment.
            uri.rsplit(['/', '#']).next().unwrap_or(uri).to_string()
        });

    let is_instrument = graph.has_type(uri, ttl::LV2_INSTRUMENT);

    let mut ports = Vec::new();
    for port_node in graph.objects(uri, ttl::LV2_PORT) {
        let pid = port_node.as_str();
        let port = parse_port(graph, pid);
        ports.push(port);
    }
    ports.sort_by_key(|p| p.index);

    let has_audio_out = ports.iter().any(|p| p.kind == PortKind::AudioOutput);
    let has_audio_in = ports.iter().any(|p| p.kind == PortKind::AudioInput);
    // An effect processes audio in→out; an instrument has audio out + MIDI in.
    let is_effect = has_audio_in && has_audio_out && !is_instrument;

    let required_features = graph
        .objects(uri, ttl::LV2_REQUIRED_FEATURE)
        .iter()
        .map(|n| n.as_str().to_string())
        .collect();

    let x11_ui = find_x11_ui(graph, uri);

    Lv2PluginInfo {
        uri: uri.to_string(),
        name,
        bundle_dir: bundle_dir.to_path_buf(),
        binary_path,
        is_instrument,
        is_effect,
        ports,
        required_features,
        x11_ui,
    }
}

/// Find the X11 editor belonging to `plugin_uri`.
///
/// Bundles link the two in either direction and plenty do neither, so all three
/// are tried in turn: `plugin ui:ui <ui>`, `ui lv2:appliesTo <plugin>`, and
/// finally — only when the bundle describes a single X11 UI — that one. The
/// fallback is what covers DPF bundles (Zam, Dragonfly), where the UI is named
/// `<plugin>#DPF_UI` and nothing states the relation at all.
fn find_x11_ui(graph: &ttl::Graph, plugin_uri: &str) -> Option<Lv2UiInfo> {
    if UI_DENY_PREFIXES.iter().any(|p| plugin_uri.starts_with(p)) {
        return None;
    }
    let x11: Vec<String> = graph.subjects_of_type(ttl::UI_X11UI);
    if x11.is_empty() {
        return None;
    }

    let declared = graph
        .objects(plugin_uri, ttl::UI_UI)
        .iter()
        .map(|n| n.as_str().to_string())
        .find(|u| x11.contains(u));

    let applies = || {
        x11.iter()
            .find(|ui| {
                graph
                    .objects(ui, ttl::LV2_APPLIES_TO)
                    .iter()
                    .any(|n| n.as_str() == plugin_uri)
            })
            .cloned()
    };

    // Only unambiguous when the bundle has exactly one: a multi-plugin bundle
    // would otherwise hand every plugin the same editor.
    let only_one = || (x11.len() == 1).then(|| x11[0].clone());

    let ui_uri = declared.or_else(applies).or_else(only_one)?;
    let binary_path = graph.object(&ui_uri, ttl::UI_BINARY).and_then(node_to_path)?;
    // A UI whose binary is missing is worse than no UI: the button would offer a
    // window that can never open.
    if !binary_path.exists() {
        return None;
    }

    // The UI's own description usually lives in a separate document, reachable
    // through its `rdfs:seeAlso`; that is where `requiredFeature` sits. Only
    // that document is parsed — merging it into a copy of the bundle graph made
    // discovery quadratic and hung the scan on big bundles (guitarix, Calf).
    let mut required: Vec<String> = graph
        .objects(&ui_uri, ttl::LV2_REQUIRED_FEATURE)
        .iter()
        .map(|n| n.as_str().to_string())
        .collect();
    for see in graph.objects(&ui_uri, ttl::RDFS_SEE_ALSO) {
        let Some(p) = node_to_path(see) else { continue };
        let Ok(g) = ttl::Graph::parse_file(&p) else { continue };
        required.extend(
            g.objects(&ui_uri, ttl::LV2_REQUIRED_FEATURE)
                .iter()
                .map(|n| n.as_str().to_string()),
        );
    }

    for uri in &required {
        if !SUPPORTED_UI_FEATURES.contains(&uri.as_str()) {
            debug!("LV2: UI {ui_uri} needs unsupported feature {uri}; no editor");
            return None;
        }
    }

    Some(Lv2UiInfo { uri: ui_uri, binary_path })
}

fn parse_port(graph: &Graph, pid: &str) -> Port {
    let index = graph
        .object(pid, ttl::LV2_INDEX)
        .and_then(|n| n.as_str().parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    let symbol = graph
        .object(pid, ttl::LV2_SYMBOL)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default();
    let name = graph
        .object(pid, ttl::LV2_NAME)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| symbol.clone());

    let is_input = graph.has_type(pid, ttl::LV2_INPUT_PORT);
    let is_output = graph.has_type(pid, ttl::LV2_OUTPUT_PORT);
    let is_audio = graph.has_type(pid, ttl::LV2_AUDIO_PORT);
    let is_control = graph.has_type(pid, ttl::LV2_CONTROL_PORT);
    let is_atom = graph.has_type(pid, ttl::ATOM_PORT);

    let kind = match (is_audio, is_control, is_atom, is_input, is_output) {
        (true, _, _, true, _) => PortKind::AudioInput,
        (true, _, _, _, true) => PortKind::AudioOutput,
        (_, true, _, true, _) => PortKind::ControlInput,
        (_, true, _, _, true) => PortKind::ControlOutput,
        (_, _, true, true, _) => PortKind::AtomInput,
        (_, _, true, _, true) => PortKind::AtomOutput,
        _ => PortKind::Unknown,
    };

    let is_midi = is_atom
        && graph
            .objects(pid, ttl::ATOM_SUPPORTS)
            .iter()
            .any(|n| n.as_str() == ttl::MIDI_EVENT);

    let default = graph
        .object(pid, ttl::LV2_DEFAULT)
        .and_then(|n| n.as_str().parse::<f32>().ok())
        .unwrap_or(0.0);
    let min = graph
        .object(pid, ttl::LV2_MINIMUM)
        .and_then(|n| n.as_str().parse::<f32>().ok())
        .unwrap_or(0.0);
    let max = graph
        .object(pid, ttl::LV2_MAXIMUM)
        .and_then(|n| n.as_str().parse::<f32>().ok())
        .unwrap_or(1.0);

    Port {
        index,
        symbol,
        name,
        kind,
        is_midi,
        default,
        min,
        max,
    }
}

/// Resolve a TTL object node into a filesystem path (it is a resolved `file://`
/// IRI when the source was a relative `<…>` ref).
fn node_to_path(node: &Node) -> Option<PathBuf> {
    match node {
        Node::Iri(iri) => ttl::file_uri_to_path(iri),
        _ => None,
    }
}
