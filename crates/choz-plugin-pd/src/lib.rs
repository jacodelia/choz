//! Pure Data patches as choz effects.
//!
//! # What was measured before any of this was written
//!
//! The roadmap said the first thing to try is the thing that decides whether
//! the rest is worth doing: load libpd, run a patch over a block, and see what
//! it costs. Done, on the machine this was written on (Debian's libpd 0.56.2):
//!
//! * A gain patch (`adc~ → *~ 0.5 → dac~`) costs **0.03 % of the audio
//!   callback** at 128, 256 and 512 frames. Pure Data is not the expensive
//!   part of anything.
//! * **No allocation per block.** Pd allocates when the DSP graph *changes* —
//!   a patch opened, DSP toggled, an object created — and not while it runs.
//!   So the rule is the one choz already lives by: build off the audio thread,
//!   process on it.
//! * **`libpd_new_instance()` returns null.** Debian's build has no
//!   `PDINSTANCE`, so there is exactly **one Pd per process**, and that is not
//!   a detail — it is the architecture. Two Pd effects in one choz cannot both
//!   exist. Each one has to be its own process.
//!
//! # Which is why this looks like a plugin
//!
//! choz already runs other people's code in its own process, with audio over
//! shared memory and a supervisor that restarts it when it dies
//! (`choz-plugin-sandbox`). A Pd effect is that, with libpd instead of a plugin
//! binary. Nothing new has to be invented for it, and two things fall out for
//! free:
//!
//! * **The licence stays clean.** libpd is LGPL and choz is MIT. The child
//!   process links libpd; the choz binary does not link it at all.
//! * **A patch that wedges Pd takes down a patch, not the session** — the same
//!   promise the plugin sandbox already makes.
//!
//! # Feature-gated
//!
//! Building choz must not require Pure Data to be installed. Without the `pd`
//! feature this crate still compiles, [`Patch::open`] says why it cannot, and
//! everything above is documentation of what to build next.

use std::path::{Path, PathBuf};

/// What a patch says it needs from the host, read from the file itself.
///
/// A `.pd` file is line-based text (`#X obj <x> <y> <name> <args>;`), so this
/// needs no Pd at all — and it is needed whichever way the patch ends up being
/// run: to wire a patch up you have to know how many channels it takes and
/// gives, and to offer it in a list you have to know it is loadable before
/// starting a process for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchInfo {
    /// Name shown in the picker: the file stem.
    pub name: String,
    /// `adc~` present: the patch reads the host's input.
    pub takes_audio: bool,
    /// `dac~` present: the patch writes to the host's output.
    pub gives_audio: bool,
    /// `notein`/`midiin` present: the patch wants note events. That is what
    /// makes a patch an **input algorithm** rather than an effect.
    pub takes_notes: bool,
    /// `noteout`/`midiout` present: the patch produces note events.
    pub gives_notes: bool,
    /// Objects the patch uses, in the order they appear. Kept because "why
    /// will this patch not load" is otherwise unanswerable.
    pub objects: Vec<String>,
}

impl PatchInfo {
    /// What choz can do with this patch, if anything.
    pub fn role(&self) -> PatchRole {
        match (
            self.takes_audio || self.gives_audio,
            self.takes_notes || self.gives_notes,
        ) {
            // Notes out is what an input algorithm is, whether it got there
            // from audio (a tracker) or from notes (an arpeggiator).
            (_, true) if self.gives_notes => PatchRole::InputAlgorithm,
            (true, _) => PatchRole::Effect,
            _ => PatchRole::Unusable,
        }
    }
}

/// Where a patch belongs in choz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchRole {
    /// Audio in, audio out: an entry in the FX chain.
    Effect,
    /// Notes out: an entry in the input-algorithm section, next to the
    /// arpeggiator.
    InputAlgorithm,
    /// Neither, so there is nothing to connect it to. Listed with the reason
    /// rather than hidden: "my patch does not appear" is the worst failure.
    Unusable,
}

/// Read a `.pd` file and say what it is.
///
/// Deliberately forgiving: an object this does not know about is recorded and
/// not rejected. The question here is "what does this patch connect to", not
/// "is this patch valid" — Pd itself answers the second one, and it answers it
/// better.
pub fn read_patch(path: &Path) -> anyhow::Result<PatchInfo> {
    let text = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "patch".to_string());
    let mut info = PatchInfo {
        name,
        ..Default::default()
    };
    // Pd escapes a real semicolon as `\;`, and statements are separated by the
    // unescaped ones. Lines wrap, so the file is one stream of statements and
    // not one statement per line.
    for statement in split_statements(&text) {
        let mut words = statement.split_whitespace();
        // `#X obj <x> <y> <name> …` is the only shape that declares an object.
        if words.next() != Some("#X") {
            continue;
        }
        if words.next() != Some("obj") {
            continue;
        }
        // Past the two coordinates is the object's name.
        let (_x, _y) = (words.next(), words.next());
        let Some(object) = words.next() else { continue };
        match object {
            "adc~" => info.takes_audio = true,
            "dac~" => info.gives_audio = true,
            "notein" | "midiin" => info.takes_notes = true,
            "noteout" | "midiout" => info.gives_notes = true,
            _ => {}
        }
        info.objects.push(object.to_string());
    }
    Ok(info)
}

/// Split a Pd file into statements on unescaped semicolons.
fn split_statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in text.chars() {
        match c {
            '\\' if !escaped => {
                escaped = true;
                current.push(c);
            }
            ';' if !escaped => {
                out.push(std::mem::take(&mut current));
            }
            _ => {
                escaped = false;
                // Newlines inside a statement are continuations, not breaks.
                current.push(if c == '\n' { ' ' } else { c });
            }
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Every `.pd` file under `dir`, one level deep.
pub fn scan_directory(dir: &Path) -> Vec<(PathBuf, PatchInfo)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pd") {
            if let Ok(info) = read_patch(&path) {
                out.push((path, info));
            }
        }
    }
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    out
}

/// A patch loaded into this process's one Pd.
///
/// **One per process.** Not a rule this crate invented — `libpd_new_instance`
/// returns null on a build without `PDINSTANCE`, which is what Debian ships,
/// so a second one would silently share the first one's DSP graph. The type
/// refuses instead: the caller is meant to be a child process with one job.
#[derive(Debug)]
pub struct Patch {
    #[allow(dead_code)]
    info: PatchInfo,
}

impl Patch {
    /// Open `path` in this process's Pd, at `sample_rate`, and switch DSP on.
    pub fn open(path: &Path, sample_rate: u32) -> anyhow::Result<Self> {
        let info = read_patch(path)?;
        #[cfg(feature = "pd")]
        {
            imp::open(path, sample_rate)?;
            Ok(Self { info })
        }
        #[cfg(not(feature = "pd"))]
        {
            let _ = sample_rate;
            anyhow::bail!(
                "'{}' needs Pure Data support, which this build does not have \
                 (rebuild with --features pd, and libpd installed)",
                info.name
            )
        }
    }

    pub fn info(&self) -> &PatchInfo {
        &self.info
    }

    /// Process one interleaved stereo block in place.
    #[cfg(feature = "pd")]
    pub fn process(&mut self, buf: &mut [f32]) {
        imp::process(buf);
    }
}

#[cfg(feature = "pd")]
mod imp {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[link(name = "pd")]
    unsafe extern "C" {
        fn libpd_init() -> c_int;
        fn libpd_init_audio(inc: c_int, outc: c_int, sr: c_int) -> c_int;
        fn libpd_openfile(name: *const c_char, dir: *const c_char) -> *mut c_void;
        fn libpd_blocksize() -> c_int;
        fn libpd_process_float(ticks: c_int, inbuf: *const f32, outbuf: *mut f32) -> c_int;
        fn libpd_start_message(maxlen: c_int) -> c_int;
        fn libpd_add_float(x: f32);
        fn libpd_finish_message(recv: *const c_char, msg: *const c_char) -> c_int;
    }

    static STARTED: AtomicBool = AtomicBool::new(false);

    pub(super) fn open(path: &Path, sample_rate: u32) -> anyhow::Result<()> {
        if STARTED.swap(true, Ordering::SeqCst) {
            anyhow::bail!(
                "this process already has a patch open, and libpd has one Pd per process"
            );
        }
        let dir = path.parent().unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let c_name = CString::new(name)?;
        let c_dir = CString::new(dir.to_string_lossy().into_owned())?;
        unsafe {
            libpd_init();
            libpd_init_audio(2, 2, sample_rate as c_int);
            if libpd_openfile(c_name.as_ptr(), c_dir.as_ptr()).is_null() {
                anyhow::bail!("Pure Data would not open {}", path.display());
            }
            // **Without this Pd computes nothing.** Measured the hard way: the
            // first run of the probe reported a wonderfully low cost and an
            // output of exactly zero.
            libpd_start_message(1);
            libpd_add_float(1.0);
            let pd = CString::new("pd")?;
            let dsp = CString::new("dsp")?;
            libpd_finish_message(pd.as_ptr(), dsp.as_ptr());
        }
        Ok(())
    }

    /// Pd works in ticks of `libpd_blocksize()` (64) frames. A block that is
    /// not a whole number of ticks is processed up to the last whole one and
    /// the remainder passed through, which is what a 100-frame buffer does on
    /// a graph that only speaks in 64s.
    pub(super) fn process(buf: &mut [f32]) {
        let frames = buf.len() / 2;
        let block = unsafe { libpd_blocksize() }.max(1) as usize;
        let ticks = frames / block;
        if ticks == 0 {
            return;
        }
        let n = ticks * block * 2;
        // Pd reads and writes the same shape choz uses: interleaved stereo,
        // but not in place — hence the copy. In the child process this becomes
        // a buffer owned once; here it is a copy per block on purpose, because
        // this path is a test and a measurement, not the audio thread.
        let input: Vec<f32> = buf[..n].to_vec();
        unsafe {
            libpd_process_float(ticks as c_int, input.as_ptr(), buf[..n].as_mut_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A patch is a text file, so what it connects to can be read without Pure
    /// Data being installed at all — which is what lets choz list patches, and
    /// say why one is not listed, on a machine that cannot run them.
    #[test]
    fn a_patch_says_what_it_connects_to() {
        let dir = std::env::temp_dir().join("choz-pd-read");
        std::fs::create_dir_all(&dir).unwrap();

        let fx = write(
            &dir,
            "gain.pd",
            "#N canvas 0 0 450 300 12;\n\
             #X obj 50 50 adc~;\n\
             #X obj 50 100 *~ 0.5;\n\
             #X obj 50 150 dac~;\n\
             #X connect 0 0 1 0;\n",
        );
        let info = read_patch(&fx).unwrap();
        assert_eq!(info.name, "gain");
        assert!(info.takes_audio && info.gives_audio);
        assert!(!info.gives_notes);
        assert_eq!(info.role(), PatchRole::Effect);
        assert!(info.objects.iter().any(|o| o == "*~"), "{:?}", info.objects);

        // Notes out is an input algorithm, wherever the notes came from.
        let algo = write(
            &dir,
            "arp.pd",
            "#N canvas 0 0 450 300 12;\n#X obj 20 20 notein;\n#X obj 20 60 noteout;\n",
        );
        let info = read_patch(&algo).unwrap();
        assert_eq!(info.role(), PatchRole::InputAlgorithm);

        // Neither: there is nothing to wire it to, and saying so beats hiding it.
        let dud = write(
            &dir,
            "gui.pd",
            "#N canvas 0 0 450 300 12;\n#X obj 20 20 bng 15 250 50 0;\n",
        );
        assert_eq!(read_patch(&dud).unwrap().role(), PatchRole::Unusable);

        // And the directory scan finds all three, by name.
        let found = scan_directory(&dir);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0].1.name, "arp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pd wraps long lines and escapes real semicolons, so a statement is not
    /// a line. A parser that assumed it was would read half an object name.
    #[test]
    fn statements_are_semicolons_not_newlines() {
        let dir = std::env::temp_dir().join("choz-pd-wrap");
        std::fs::create_dir_all(&dir).unwrap();
        let path = write(
            &dir,
            "wrapped.pd",
            "#N canvas 0 0 450 300 12;\n\
             #X obj 50 50\n adc~;\n\
             #X text 10 10 a semicolon \\; inside a comment;\n\
             #X obj 50 150 dac~;\n",
        );
        let info = read_patch(&path).unwrap();
        assert!(
            info.takes_audio && info.gives_audio,
            "a wrapped statement is still one statement: {:?}",
            info.objects
        );
        assert_eq!(info.role(), PatchRole::Effect);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing, end to end: a gain patch loaded into libpd, a block
    /// through it, and the number that comes out. Only with the feature on and
    /// Pure Data installed — everywhere else the crate still builds and the
    /// test above covers what it says instead.
    #[cfg(feature = "pd")]
    #[test]
    fn a_patch_processes_a_block() {
        let dir = std::env::temp_dir().join("choz-pd-run");
        std::fs::create_dir_all(&dir).unwrap();
        let path = write(
            &dir,
            "gain.pd",
            "#N canvas 0 0 450 300 12;\n\
             #X obj 50 50 adc~;\n\
             #X obj 50 100 *~ 0.5;\n\
             #X obj 50 150 dac~;\n\
             #X connect 0 0 1 0;\n\
             #X connect 1 0 2 0;\n\
             #X connect 1 0 2 1;\n",
        );
        let mut patch = Patch::open(&path, 48_000).expect("libpd opened the patch");
        assert_eq!(patch.info().role(), PatchRole::Effect);

        let mut buf = vec![0.4f32; 128 * 2];
        patch.process(&mut buf);
        assert!(
            (buf[0] - 0.2).abs() < 1e-5,
            "the patch halves it: {} (DSP off would give exactly 0)",
            buf[0]
        );

        // One Pd per process, and the type says so rather than silently
        // sharing the first patch's DSP graph.
        let err = Patch::open(&path, 48_000).unwrap_err().to_string();
        assert!(err.contains("one Pd per process"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without the feature the crate still builds and still reads patches; it
    /// just says why it cannot run one. A host that will not build without
    /// Pure Data installed is a host nobody can build.
    #[cfg(not(feature = "pd"))]
    #[test]
    fn without_the_feature_it_says_so_instead_of_pretending() {
        let dir = std::env::temp_dir().join("choz-pd-nofeature");
        std::fs::create_dir_all(&dir).unwrap();
        let path = write(&dir, "gain.pd", "#N canvas 0 0 1 1 12;\n#X obj 0 0 dac~;\n");
        let err = Patch::open(&path, 48_000).unwrap_err().to_string();
        assert!(err.contains("--features pd"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
