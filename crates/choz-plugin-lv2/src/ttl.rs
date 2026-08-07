//! Minimal RDF/Turtle querying for LV2 bundles.
//!
//! We parse a `.ttl` file into an owned list of triples with `rio_turtle`, then
//! run small queries over it. Relative IRIs (e.g. `<plugin.so>`) are resolved
//! against the file's own `file://` base, so object IRIs come back absolute and
//! can be turned straight into filesystem paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rio_api::model::{Subject, Term};
use rio_api::parser::TriplesParser;
use rio_turtle::TurtleParser;
macro_rules! debug { ($($t:tt)*) => {{ if false { eprintln!($($t)*); } }}; }

// ─── Well-known LV2 vocabulary IRIs ─────────────────────────────────────────

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const RDFS_SEE_ALSO: &str = "http://www.w3.org/2000/01/rdf-schema#seeAlso";
pub const LV2_PLUGIN: &str = "http://lv2plug.in/ns/lv2core#Plugin";
pub const LV2_INSTRUMENT: &str = "http://lv2plug.in/ns/lv2core#InstrumentPlugin";
pub const LV2_BINARY: &str = "http://lv2plug.in/ns/lv2core#binary";
pub const LV2_PORT: &str = "http://lv2plug.in/ns/lv2core#port";
pub const LV2_INDEX: &str = "http://lv2plug.in/ns/lv2core#index";
pub const LV2_SYMBOL: &str = "http://lv2plug.in/ns/lv2core#symbol";
pub const LV2_NAME: &str = "http://lv2plug.in/ns/lv2core#name";
pub const LV2_DEFAULT: &str = "http://lv2plug.in/ns/lv2core#default";
pub const LV2_MINIMUM: &str = "http://lv2plug.in/ns/lv2core#minimum";
pub const LV2_MAXIMUM: &str = "http://lv2plug.in/ns/lv2core#maximum";
pub const LV2_INPUT_PORT: &str = "http://lv2plug.in/ns/lv2core#InputPort";
pub const LV2_OUTPUT_PORT: &str = "http://lv2plug.in/ns/lv2core#OutputPort";
pub const LV2_AUDIO_PORT: &str = "http://lv2plug.in/ns/lv2core#AudioPort";
pub const LV2_CONTROL_PORT: &str = "http://lv2plug.in/ns/lv2core#ControlPort";
pub const ATOM_PORT: &str = "http://lv2plug.in/ns/ext/atom#AtomPort";
pub const ATOM_SUPPORTS: &str = "http://lv2plug.in/ns/ext/atom#supports";
pub const MIDI_EVENT: &str = "http://lv2plug.in/ns/ext/midi#MidiEvent";
pub const DOAP_NAME: &str = "http://usefulinc.com/ns/doap#name";
pub const LV2_REQUIRED_FEATURE: &str = "http://lv2plug.in/ns/lv2core#requiredFeature";
pub const UI_X11UI: &str = "http://lv2plug.in/ns/extensions/ui#X11UI";
pub const UI_BINARY: &str = "http://lv2plug.in/ns/extensions/ui#binary";
pub const UI_UI: &str = "http://lv2plug.in/ns/extensions/ui#ui";
pub const LV2_APPLIES_TO: &str = "http://lv2plug.in/ns/lv2core#appliesTo";

/// Monotonic salt source for namespacing blank-node labels across parsed files.
static NEXT_BLANK_SALT: AtomicU64 = AtomicU64::new(0);

// ─── Owned triple store ─────────────────────────────────────────────────────

/// A subject or object node, reduced to the two cases we care about plus literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Iri(String),
    Blank(String),
    Lit(String),
}

impl Node {
    pub fn as_str(&self) -> &str {
        match self {
            Node::Iri(s) | Node::Blank(s) | Node::Lit(s) => s,
        }
    }
}

/// One parsed triple with owned strings (predicate is always an IRI).
#[derive(Debug, Clone)]
pub struct Triple {
    pub s: Node,
    pub p: String,
    pub o: Node,
}

/// An in-memory triple store for one or more `.ttl` documents.
///
/// A `by_subject` index maps each subject string to the indices of its triples,
/// so per-subject queries (the common case for LV2 plugin/port lookups) avoid a
/// full linear scan of the whole graph. The index is built lazily by
/// [`Graph::index`]; mutating `triples` directly invalidates it until rebuilt.
#[derive(Default)]
pub struct Graph {
    pub triples: Vec<Triple>,
    by_subject: HashMap<String, Vec<usize>>,
}

impl Graph {
    /// Build (or rebuild) the subject index after `triples` is fully populated.
    pub fn index(&mut self) {
        let mut idx: HashMap<String, Vec<usize>> = HashMap::with_capacity(self.triples.len());
        for (i, t) in self.triples.iter().enumerate() {
            idx.entry(t.s.as_str().to_string()).or_default().push(i);
        }
        self.by_subject = idx;
    }

    /// Merge another graph's triples into this one (index rebuilt by caller).
    pub fn extend_from(&mut self, other: &Graph) {
        self.triples.extend(other.triples.iter().cloned());
    }

    /// Parse a `.ttl` file, resolving relative IRIs against the file's location.
    pub fn parse_file(path: &Path) -> anyhow::Result<Graph> {
        let text = std::fs::read_to_string(path)?;
        let base = path_to_file_uri(path);
        let base_iri = oxiri::Iri::parse(base).ok();
        let mut parser = TurtleParser::new(std::io::Cursor::new(text.into_bytes()), base_iri);
        let mut triples = Vec::new();
        // rio borrows term strings from an internal buffer; copy to owned nodes.
        let res = parser.parse_all(&mut |t| {
            let s = subject_to_node(t.subject);
            let p = t.predicate.iri.to_string();
            let o = term_to_node(t.object);
            triples.push(Triple { s, p, o });
            Ok(()) as Result<(), rio_turtle::TurtleError>
        });
        if let Err(e) = res {
            debug!("TTL parse error in {}: {e}", path.display());
        }
        // Blank-node labels are document-local and `rio` reuses them across
        // separate parses (e.g. `_:b1`). When several `.ttl` files are merged
        // into one bundle graph, those labels would collide and cross-link
        // unrelated subjects (silently dropping ports). Namespace every blank
        // label with a globally-unique per-file token to keep them distinct.
        let salt = NEXT_BLANK_SALT.fetch_add(1, Ordering::Relaxed);
        for t in &mut triples {
            if let Node::Blank(id) = &mut t.s {
                *id = format!("b{salt}:{id}");
            }
            if let Node::Blank(id) = &mut t.o {
                *id = format!("b{salt}:{id}");
            }
        }
        let mut g = Graph { triples, by_subject: HashMap::new() };
        g.index();
        Ok(g)
    }

    /// All objects for `(subject, predicate)`, using the subject index.
    pub fn objects(&self, subject: &str, predicate: &str) -> Vec<&Node> {
        match self.by_subject.get(subject) {
            Some(idxs) => idxs
                .iter()
                .map(|&i| &self.triples[i])
                .filter(|t| t.p == predicate)
                .map(|t| &t.o)
                .collect(),
            None => Vec::new(),
        }
    }

    /// First object for `(subject, predicate)`, if any.
    pub fn object(&self, subject: &str, predicate: &str) -> Option<&Node> {
        self.objects(subject, predicate).into_iter().next()
    }

    /// Whether `(subject rdf:type type_iri)` is asserted.
    pub fn has_type(&self, subject: &str, type_iri: &str) -> bool {
        self.objects(subject, RDF_TYPE)
            .iter()
            .any(|o| o.as_str() == type_iri)
    }

    /// All subjects asserted to be `(rdf:type type_iri)`.
    pub fn subjects_of_type(&self, type_iri: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .triples
            .iter()
            .filter(|t| t.p == RDF_TYPE && t.o.as_str() == type_iri)
            .map(|t| t.s.as_str().to_string())
            .collect();
        out.dedup();
        out
    }
}

fn subject_to_node(s: Subject) -> Node {
    match s {
        Subject::NamedNode(n) => Node::Iri(n.iri.to_string()),
        Subject::BlankNode(b) => Node::Blank(b.id.to_string()),
        Subject::Triple(_) => Node::Blank(String::new()), // rdf-star: unused
    }
}

fn term_to_node(o: Term) -> Node {
    match o {
        Term::NamedNode(n) => Node::Iri(n.iri.to_string()),
        Term::BlankNode(b) => Node::Blank(b.id.to_string()),
        Term::Literal(l) => Node::Lit(literal_value(l)),
        Term::Triple(_) => Node::Blank(String::new()),
    }
}

fn literal_value(l: rio_api::model::Literal) -> String {
    use rio_api::model::Literal::*;
    match l {
        Simple { value } => value.to_string(),
        LanguageTaggedString { value, .. } => value.to_string(),
        Typed { value, .. } => value.to_string(),
    }
}

/// Convert an absolute filesystem path into a `file://` URI string. The path is
/// percent-encoded so bundle directories containing spaces or other characters
/// that are illegal in an IRI (e.g. `Surge XT Effects.lv2`) still produce a
/// valid base IRI that `oxiri` accepts — otherwise relative `<plugin.so>`
/// references in the TTL fail to resolve and the plugin is dropped.
pub fn path_to_file_uri(path: &Path) -> String {
    // Avoid the per-file `canonicalize` syscall when the path is already
    // absolute (the scan always hands us absolute bundle paths).
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    };
    format!("file://{}", percent_encode_path(&abs.to_string_lossy()))
}

/// Convert a resolved `file://` IRI (object of a relative ref) back to a path,
/// percent-decoding it (e.g. `%20` → space) to recover the real filesystem path.
pub fn file_uri_to_path(iri: &str) -> Option<PathBuf> {
    iri.strip_prefix("file://").map(|s| PathBuf::from(percent_decode(s)))
}

/// Percent-encode a filesystem path for use in a `file://` IRI. `/` is kept as
/// the path separator; unreserved characters pass through; everything else is
/// `%XX`-encoded.
fn percent_encode_path(s: &str) -> String {
    fn unreserved(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
    }
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap().to_ascii_uppercase());
            out.push(char::from_digit((b & 0xf) as u32, 16).unwrap().to_ascii_uppercase());
        }
    }
    out
}

/// Percent-decode an IRI path (`%XX` → byte), leaving other characters intact.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_roundtrip_with_spaces() {
        // A bundle path with spaces (e.g. "Surge XT Effects.lv2") must produce a
        // valid base IRI and decode back to the exact original path.
        let p = Path::new("/usr/lib/lv2/Surge XT Effects.lv2/manifest.ttl");
        let uri = path_to_file_uri(p);
        assert!(uri.starts_with("file:///usr/lib/lv2/Surge%20XT%20Effects.lv2/"));
        assert!(oxiri::Iri::parse(uri.as_str()).is_ok(), "base IRI must be valid");
        assert_eq!(file_uri_to_path(&uri).unwrap(), p);
    }

    #[test]
    fn decode_encoded_binary_ref() {
        // The object that comes back from resolving `<libSurge%20XT%20Effects.so>`.
        let iri = "file:///usr/lib/lv2/Surge%20XT%20Effects.lv2/libSurge%20XT%20Effects.so";
        let path = file_uri_to_path(iri).unwrap();
        assert_eq!(
            path,
            Path::new("/usr/lib/lv2/Surge XT Effects.lv2/libSurge XT Effects.so")
        );
    }
}
