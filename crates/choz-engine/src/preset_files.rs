//! A folder of preset **files** as a plugin's bank.
//!
//! Some formats hand their patches over: CLAP has a preset-discovery factory,
//! VST3 has `IUnitInfo` program lists, a DSSI synth has programs. Plenty of
//! plugins have none of that and keep their sounds on disk instead — Surge XT
//! ships 3000 `.fxp` files under `/usr/share/surge-xt`, and its VST3 build
//! reports **no** programs at all, so a rack tab holding it had no bank and no
//! way to reach any of them.
//!
//! What every one of those files carries is the same blob the plugin's own
//! state call produces, inside a container header:
//!
//! * `.fxp` / `.fxb` — VST2's; `FPCh`/`FBCh` are the opaque-chunk kinds and the
//!   patch starts right after the header. (`FxCk`/`FxBk` are lists of parameter
//!   values instead, which is not a state blob and is refused as such.)
//! * `.vstpreset` — VST3's; a chunk list whose `Comp` entry *is*
//!   `IComponent::getState`.
//!
//! So a bank is a directory, a preset is a file, and loading one is
//! [`choz_ports::PluginState::restore`] — the same call a project load makes.

use anyhow::{bail, Context, Result};
use std::path::Path;

use choz_ports::PresetEntry;

/// What counts as a preset file.
///
/// The first three are containers with a header around the plugin's own state.
/// `.h2p` is not a container at all: u-he's plugins keep their patches as text
/// files and their state chunk **is** that text, so the file goes to the plugin
/// exactly as it is on disk (checked against TyrellN6: 2775 bytes in, the state
/// changes, and the patch sounds).
pub const PRESET_EXTS: &[&str] = &["fxp", "fxb", "vstpreset", "h2p"];

/// How deep under the bank folder to look. Patch libraries are filed by
/// category, sometimes twice (`Factory / Basses`); past that it is somebody's
/// whole disk.
const MAX_DEPTH: usize = 4;

/// Every preset file under `dir`, as bank entries: the sub-folder is the
/// category (which the picker turns into its chips), the file stem the name,
/// and the full path the key.
pub fn list_bank(dir: &Path) -> Vec<PresetEntry> {
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PresetEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, out);
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !PRESET_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let category = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().replace('/', " \u{00B7} "))
            .unwrap_or_default();
        out.push(PresetEntry {
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            category,
            key: path.to_string_lossy().into_owned(),
        });
    }
}

/// Where a plugin keeps its patches, guessed from its name.
///
/// A plugin that publishes no programs still ships its sounds somewhere, and
/// asking the user to go and find them is asking them to know where a package
/// put its data: Surge XT's are 637 `.fxp` files under
/// `/usr/share/surge-xt/patches_factory`, filed by category, and its own window
/// shows them as `Leads / Butter`. This looks in the places a Linux package (or
/// the user's own folder) puts them, matching on the plugin's name with the
/// punctuation taken out — "Surge XT" finds `surge-xt`.
///
/// It never guesses *what* is in a file: the directory it returns is one whose
/// files actually parse as this format's presets, or it returns nothing and the
/// folder picker is one keypress away.
pub fn guess_bank_dir(name: &str, plugin_path: &Path) -> Option<std::path::PathBuf> {
    let want = squash(name);
    if want.is_empty() {
        return None;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    // Nearest first: what ships with the plugin, then the user's own folders,
    // then the system's. A plugin installed twice should answer with the copy
    // it was loaded from.
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    roots.push(plugin_path.to_path_buf());
    if let Some(parent) = plugin_path.parent() {
        roots.push(parent.to_path_buf());
    }
    if let Some(h) = &home {
        roots.push(h.join(".local/share"));
        roots.push(h.join("Documents"));
        roots.push(h.clone());
    }
    roots.extend([
        std::path::PathBuf::from("/usr/local/share"),
        std::path::PathBuf::from("/usr/share"),
        std::path::PathBuf::from("/opt"),
    ]);

    for root in roots {
        // Two levels, because vendors file their data by vendor first: u-he
        // puts TyrellN6's patches in `~/.u-he/TyrellN6`, and `.u-he` answers to
        // no plugin's name.
        for dir in children(&root) {
            if matches(&dir, &want) {
                if let Some(found) = best_bank_dir(&dir, 0) {
                    return Some(found);
                }
            }
            for inner in children(&dir) {
                if matches(&inner, &want) {
                    if let Some(found) = best_bank_dir(&inner, 0) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

/// Sub-directories of `dir`, or nothing at all when it cannot be read.
fn children(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether this directory answers to the plugin's name, either way round:
/// `surge-xt` for "Surge XT", `TyrellN6` for "TyrellN6".
fn matches(dir: &Path, want: &str) -> bool {
    let Some(name) = dir.file_name().map(|n| squash(&n.to_string_lossy())) else {
        return false;
    };
    !name.is_empty() && (name == want || name.contains(want) || want.contains(&name))
}

/// Lowercase, letters and digits only — so "Surge XT" and "surge-xt" are the
/// same word, which is the whole point of matching on a name at all.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The directory under `dir` that a bank should show — the one whose
/// sub-folders are the categories the plugin's own browser names.
///
/// The rules come from what plugins actually ship:
///
/// * **Presets right here** → this is it.
/// * **One sub-folder has them** → there was no choice to make, so keep going
///   down. u-he's layout is `~/.u-he/TyrellN6/Presets/TyrellN6/01 Basses/…`, and
///   both of those middle levels are single doors.
/// * **Several have them** → these are the categories, so stop and show them —
///   *unless* one of them is the factory library, which is where the names
///   people know live (`patches_factory` next to `patches_3rdparty` in Surge
///   XT's data directory). Whatever is skipped is one folder-pick away.
fn best_bank_dir(dir: &Path, depth: usize) -> Option<std::path::PathBuf> {
    // A patch library is filed a few levels deep, never a dozen.
    if depth > 4 {
        return None;
    }
    let here = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| is_preset(&e.path()))
        .count();
    if here > 0 {
        return Some(dir.to_path_buf());
    }
    let mut with_presets: Vec<std::path::PathBuf> = children(dir)
        .into_iter()
        .filter(|c| !list_bank(c).is_empty())
        .collect();
    match with_presets.len() {
        0 => None,
        1 => best_bank_dir(&with_presets.remove(0), depth + 1),
        _ => with_presets
            .iter()
            .find(|c| {
                c.file_name()
                    .map(|n| squash(&n.to_string_lossy()).contains("factory"))
                    .unwrap_or(false)
            })
            .cloned()
            .or_else(|| Some(dir.to_path_buf())),
    }
}

fn is_preset(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| PRESET_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The state blob inside a preset file, ready for `PluginState::restore`.
pub fn read_state(file: &Path) -> Result<Vec<u8>> {
    let data = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    match file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "vstpreset" => vstpreset_component(&data),
        // No container: the file is the patch.
        "h2p" => Ok(data),
        _ => fx_chunk(&data),
    }
    .with_context(|| format!("{}", file.display()))
}

/// The patch inside a VST2 `.fxp` / `.fxb`.
///
/// Both start `CcnK`; the kind at bytes 8..12 says what follows. The two chunk
/// kinds are the ones that carry a plugin's own blob, and their headers are a
/// fixed size — 60 bytes for a program, 156 for a bank.
fn fx_chunk(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 16 || &data[0..4] != b"CcnK" {
        bail!("not a VST2 preset file (no CcnK)");
    }
    let header = match &data[8..12] {
        b"FPCh" => 60,
        b"FBCh" => 156,
        b"FxCk" | b"FxBk" => bail!(
            "this preset is a list of parameter values, not a patch — \
             choz can only load the chunk kinds (FPCh / FBCh)"
        ),
        other => bail!("unknown VST2 preset kind {:?}", String::from_utf8_lossy(other)),
    };
    // The declared chunk size is checked rather than trusted: a truncated file
    // handed to a plugin's `setState` is a crash in somebody else's code.
    let size = u32::from_be_bytes(data[header - 4..header].try_into()?) as usize;
    let body = data
        .get(header..)
        .filter(|b| b.len() >= size)
        .context("preset file is shorter than the chunk it declares")?;
    Ok(body[..size].to_vec())
}

/// The `Comp` chunk of a `.vstpreset`: the component state, which is exactly
/// what `IComponent::setState` wants back.
///
/// Layout: `VST3` + version + 32 chars of class id + an offset to a chunk list
/// of `(id, offset, size)` entries, all little-endian.
fn vstpreset_component(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 48 || &data[0..4] != b"VST3" {
        bail!("not a .vstpreset (no VST3 header)");
    }
    let list_at = u64::from_le_bytes(data[40..48].try_into()?) as usize;
    let list = data.get(list_at..).context("chunk list past the file")?;
    if list.len() < 8 || &list[0..4] != b"List" {
        bail!("no chunk list where the header says one is");
    }
    let count = u32::from_le_bytes(list[4..8].try_into()?) as usize;
    for i in 0..count {
        let at = 8 + i * 20;
        let Some(entry) = list.get(at..at + 20) else {
            break;
        };
        if &entry[0..4] != b"Comp" {
            continue;
        }
        let offset = u64::from_le_bytes(entry[4..12].try_into()?) as usize;
        let size = u64::from_le_bytes(entry[12..20].try_into()?) as usize;
        return data
            .get(offset..offset + size)
            .map(<[u8]>::to_vec)
            .context("the component chunk is outside the file");
    }
    bail!("no component chunk in this .vstpreset")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two containers, unwrapped to the same blob a plugin would have
    /// handed back — the whole feature is that these are interchangeable with
    /// `PluginState::save`.
    #[test]
    fn a_preset_file_gives_back_the_plugins_own_state() {
        let patch = b"sub3\x01\x02\x03 the patch".to_vec();

        // .fxp: 60 bytes of header, the last four being the chunk size.
        let mut fxp = vec![0u8; 60];
        fxp[0..4].copy_from_slice(b"CcnK");
        fxp[8..12].copy_from_slice(b"FPCh");
        fxp[56..60].copy_from_slice(&(patch.len() as u32).to_be_bytes());
        fxp.extend_from_slice(&patch);
        assert_eq!(fx_chunk(&fxp).unwrap(), patch);

        // A truncated one is refused rather than handed to a plugin.
        assert!(fx_chunk(&fxp[..fxp.len() - 4]).is_err());
        // …and so is the parameter-list kind, which carries no patch at all.
        let mut params = fxp.clone();
        params[8..12].copy_from_slice(b"FxCk");
        assert!(fx_chunk(&params).is_err());

        // .vstpreset: header, the chunk, then the list that points at it.
        let mut vp = vec![0u8; 48];
        vp[0..4].copy_from_slice(b"VST3");
        vp[40..48].copy_from_slice(&((48 + patch.len()) as u64).to_le_bytes());
        vp.extend_from_slice(&patch);
        vp.extend_from_slice(b"List");
        vp.extend_from_slice(&1u32.to_le_bytes());
        vp.extend_from_slice(b"Comp");
        vp.extend_from_slice(&48u64.to_le_bytes());
        vp.extend_from_slice(&(patch.len() as u64).to_le_bytes());
        assert_eq!(vstpreset_component(&vp).unwrap(), patch);
    }

    /// The patches of a plugin that publishes no programs are found by name,
    /// and what comes back is the folder its own browser would show — the one
    /// with the categories in it, not the data directory above it.
    #[test]
    fn a_plugins_patches_are_found_by_its_name() {
        let root = std::env::temp_dir().join(format!("choz_guess_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // A package layout: <root>/surge-xt/{patches_factory/{Leads,Basses},
        // wavetables}, with the presets two levels down.
        let data = root.join("surge-xt");
        std::fs::create_dir_all(data.join("patches_factory/Leads")).unwrap();
        std::fs::create_dir_all(data.join("patches_factory/Basses")).unwrap();
        std::fs::create_dir_all(data.join("wavetables")).unwrap();
        std::fs::write(data.join("patches_factory/Leads/Butter.fxp"), b"x").unwrap();
        std::fs::write(data.join("patches_factory/Basses/Tok.fxp"), b"x").unwrap();
        std::fs::write(data.join("wavetables/saw.wt"), b"x").unwrap();

        let found = guess_bank_dir("Surge XT", &root.join("Surge XT.vst3"))
            .expect("the patches are found from the plugin's name");
        assert_eq!(
            found,
            data.join("patches_factory"),
            "the folder with the categories, not the data directory above it"
        );
        let bank = list_bank(&found);
        assert_eq!(bank.len(), 2);
        assert_eq!(bank[0].category, "Basses", "and the category is what the plugin calls it");

        // Nothing that answers to the name: no guess, and the folder picker is
        // still there.
        assert!(guess_bank_dir("Nothing Like It", &root.join("x.vst3")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// u-he's layout, which is nothing like Surge's: the patches are text
    /// files (`.h2p`, which *is* the state blob) under two single-door levels,
    /// `~/.u-he/<Plugin>/Presets/<Plugin>/<category>/`. Both doors are walked
    /// through, because a level with one way on is not a choice; the level with
    /// the categories is where it stops.
    #[test]
    fn a_vendors_own_layout_is_walked_down_to_the_categories() {
        let root = std::env::temp_dir().join(format!("choz_uhe_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join(".u-he/TyrellN6/Presets/TyrellN6");
        std::fs::create_dir_all(base.join("01 Basses")).unwrap();
        std::fs::create_dir_all(base.join("02 Leads")).unwrap();
        // An empty folder alongside, which is not a door at all.
        std::fs::create_dir_all(root.join(".u-he/TyrellN6/UserPresets")).unwrap();
        std::fs::write(base.join("01 Basses/Angry Dog.h2p"), b"#AM=TyrellN6").unwrap();
        std::fs::write(base.join("02 Leads/Bell Flower.h2p"), b"#AM=TyrellN6").unwrap();

        let found = best_bank_dir(&root.join(".u-he/TyrellN6"), 0).expect("the patches are there");
        assert_eq!(found, base, "stops where the categories are");
        let bank = list_bank(&found);
        assert_eq!(bank.len(), 2);
        assert_eq!(bank[0].category, "01 Basses");

        // And the file is the patch: no container, nothing to unwrap.
        let state = read_state(&base.join("01 Basses/Angry Dog.h2p")).unwrap();
        assert_eq!(state, b"#AM=TyrellN6");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A bank is a directory tree: the sub-folder is the category the picker
    /// files it under, and nothing that is not a preset comes back.
    #[test]
    fn a_folder_of_files_reads_as_a_bank() {
        let dir = std::env::temp_dir().join(format!("choz_bank_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Leads")).unwrap();
        std::fs::write(dir.join("Leads/Tok.fxp"), b"x").unwrap();
        std::fs::write(dir.join("Init.vstpreset"), b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), b"x").unwrap();

        let bank = list_bank(&dir);
        assert_eq!(bank.len(), 2, "only the preset files: {bank:?}");
        assert_eq!(bank[0].name, "Init");
        assert_eq!(bank[0].category, "", "the top level has no category");
        assert_eq!(bank[1].category, "Leads");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
