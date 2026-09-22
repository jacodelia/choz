//! The map, saved: `.smpreset`.
//!
//! A folder becomes an instrument by scanning it, reading every name, and
//! analysing whatever the names did not say. That is the right thing to do
//! once. A preset is the answer to it — every region, with its keys, its
//! velocities, its root and its tuning — written out so the next load is a
//! file read.
//!
//! It is also the only place a **corrected** map can live. The detector is
//! wrong sometimes, a pack names a file after the key it was played *from*
//! rather than the key it sounds, and the fix is a number somebody types. That
//! number has to survive being reopened, and this is where it survives.
//!
//! ## Relinking
//!
//! A library that moved is a preset full of paths that are not there any more.
//! [`relink`] matches by **file name** under a new root: the pack's own names
//! are what the whole sampler reads anyway, and two files with the same name in
//! one library are the same sample in two folders. No content hash — see the
//! note in [`super::cache`] — and no guessing past the name: a file the new root
//! does not have stays missing and is reported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::instruments::sfz::SfzRegion;

/// What a `.smpreset` is written for. Bumped when the shape changes, so an old
/// one is refused rather than half-read.
const VERSION: u32 = 1;

/// One region, as it is written down. The same fields [`SfzRegion`] carries —
/// spelled out here rather than deriving on that type, because what goes in a
/// file is a decision and not whatever the playing code happens to hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub sample: PathBuf,
    pub lo_key: u8,
    pub hi_key: u8,
    pub root: u8,
    pub lo_vel: u8,
    pub hi_vel: u8,
    pub gain: f32,
    pub tune_cents: f32,
    pub start: f32,
    pub end: f32,
    #[serde(default)]
    pub articulation: usize,
}

/// A saved instrument: where it came from, what it can be played with, and
/// every region of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub version: u32,
    /// The folder it was built from, for relinking and for saying where it went.
    pub root: PathBuf,
    /// The name the instrument plays under.
    pub name: String,
    /// The articulations, in keyswitch order. Empty for an instrument with one.
    #[serde(default)]
    pub articulations: Vec<String>,
    /// The lowest key that selects an articulation, when there are any.
    #[serde(default)]
    pub switch_base: Option<u8>,
    pub regions: Vec<Region>,
}

impl Preset {
    /// Everything a built instrument knows, ready to write.
    pub fn of(
        root: &Path,
        name: &str,
        regions: &[SfzRegion],
        articulations: &[String],
        switch_base: Option<u8>,
    ) -> Self {
        Self {
            version: VERSION,
            root: root.to_path_buf(),
            name: name.to_string(),
            articulations: articulations.to_vec(),
            switch_base,
            regions: regions.iter().map(Region::of).collect(),
        }
    }

    /// The regions, back in the shape the player takes.
    pub fn regions(&self) -> Vec<SfzRegion> {
        self.regions.iter().map(Region::to_region).collect()
    }

    /// Which files it needs and does not have.
    pub fn missing(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .regions
            .iter()
            .map(|r| r.sample.clone())
            .filter(|p| !exists(p))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

impl Region {
    fn of(r: &SfzRegion) -> Self {
        Self {
            sample: r.sample.clone(),
            lo_key: r.lo_key,
            hi_key: r.hi_key,
            root: r.pitch_key_center,
            lo_vel: r.lo_vel,
            hi_vel: r.hi_vel,
            gain: r.gain,
            tune_cents: r.tune_cents,
            start: r.start,
            end: r.end,
            articulation: r.articulation,
        }
    }

    fn to_region(&self) -> SfzRegion {
        SfzRegion {
            sample: self.sample.clone(),
            lo_key: self.lo_key,
            hi_key: self.hi_key,
            pitch_key_center: self.root,
            lo_vel: self.lo_vel,
            hi_vel: self.hi_vel,
            gain: self.gain,
            tune_cents: self.tune_cents,
            start: self.start,
            end: self.end,
            articulation: self.articulation,
        }
    }
}

/// A sample that lives inside a zip is there if the archive is — the entry is
/// checked when it is read, which is the only place it can be checked cheaply.
fn exists(path: &Path) -> bool {
    match super::archive::split(path) {
        Some((archive, _)) => archive.exists(),
        None => path.exists(),
    }
}

/// What a folder's preset is called by default: beside the folder, named after
/// it. `~/Samples/violin` → `~/Samples/violin.smpreset`.
pub fn path_for(dir: &Path) -> PathBuf {
    let mut path = dir.to_path_buf();
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "instrument".into());
    path.set_file_name(format!("{name}.smpreset"));
    path
}

/// Write one. Pretty JSON: a map somebody corrected by hand is a thing they may
/// want to read, and it diffs.
pub fn write(path: &Path, preset: &Preset) -> Result<()> {
    let json = serde_json::to_string_pretty(preset)?;
    std::fs::write(path, json).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Read one. A version this build does not know is an error rather than a guess.
pub fn read(path: &Path) -> Result<Preset> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let preset: Preset = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a sampler preset", path.display()))?;
    if preset.version != VERSION {
        bail!(
            "{}: written by another version of choz ({} rather than {VERSION})",
            path.display(),
            preset.version
        );
    }
    if preset.regions.is_empty() {
        bail!("{}: a preset with no regions in it", path.display());
    }
    Ok(preset)
}

/// Point a preset at a library that moved, matching by file name.
///
/// Returns how many regions were repointed. Whatever `new_root` does not have
/// keeps the path it had — a preset half relinked still plays the half it
/// found, and [`Preset::missing`] says what is still gone.
pub fn relink(preset: &mut Preset, new_root: &Path) -> usize {
    // Every file under the new root, by name. One walk, however many regions
    // there are: a library is thousands of files and a preset is hundreds.
    let mut by_name: HashMap<String, PathBuf> = HashMap::new();
    for (path, _) in super::files(new_root) {
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            by_name.entry(name).or_insert(path);
        }
    }
    let mut moved = 0;
    for region in &mut preset.regions {
        if exists(&region.sample) {
            continue;
        }
        let Some(name) = region
            .sample
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };
        if let Some(found) = by_name.get(&name) {
            region.sample = found.clone();
            moved += 1;
        }
    }
    if moved > 0 {
        preset.root = new_root.to_path_buf();
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(sample: &str, key: u8) -> SfzRegion {
        SfzRegion {
            sample: PathBuf::from(sample),
            lo_key: key,
            hi_key: key + 11,
            pitch_key_center: key,
            lo_vel: 0,
            hi_vel: 127,
            gain: 1.0,
            tune_cents: -12.0,
            start: 0.0,
            end: 1.0,
            articulation: 0,
        }
    }

    #[test]
    fn a_map_survives_being_written_and_read() {
        let dir = std::env::temp_dir().join(format!("choz-smpreset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let regions = vec![
            region("/lib/violin/C4.wav", 60),
            region("/lib/violin/C5.wav", 72),
        ];
        let preset = Preset::of(
            Path::new("/lib/violin"),
            "violin",
            &regions,
            &["sustain".into(), "staccato".into()],
            Some(24),
        );
        let path = dir.join("violin.smpreset");
        write(&path, &preset).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back, preset);
        // …and back into what the player takes, unchanged.
        assert_eq!(back.regions(), regions);
        assert_eq!(back.switch_base, Some(24));

        // A preset from another version is refused rather than half-read.
        let mut old = preset.clone();
        old.version = VERSION + 1;
        write(&path, &old).unwrap();
        assert!(read(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default name sits beside the folder, not inside it: a `.smpreset`
    /// inside the library would be one more file the scanner has to ignore.
    #[test]
    fn a_preset_is_named_after_its_folder() {
        assert_eq!(
            path_for(Path::new("/lib/violin")),
            Path::new("/lib/violin.smpreset")
        );
    }

    /// A library that moved: the files are found again by name, and what is
    /// genuinely gone is reported rather than silently dropped.
    #[test]
    fn a_moved_library_is_relinked_by_name() {
        let root = std::env::temp_dir().join(format!("choz-relink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let moved = root.join("violin");
        std::fs::create_dir_all(&moved).unwrap();
        // One of the two files made it; the other did not.
        std::fs::write(moved.join("C4.wav"), []).unwrap();

        let mut preset = Preset::of(
            Path::new("/gone/violin"),
            "violin",
            &[
                region("/gone/violin/C4.wav", 60),
                region("/gone/violin/C5.wav", 72),
            ],
            &[],
            None,
        );
        assert_eq!(preset.missing().len(), 2);
        assert_eq!(relink(&mut preset, &moved), 1);
        assert_eq!(preset.regions[0].sample, moved.join("C4.wav"));
        assert_eq!(preset.root, moved);
        // What is still gone says so.
        assert_eq!(preset.missing(), vec![PathBuf::from("/gone/violin/C5.wav")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
