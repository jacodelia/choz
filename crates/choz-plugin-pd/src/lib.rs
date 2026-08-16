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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatchInfo {
    /// Name shown in the picker: the file stem.
    pub name: String,
    /// `adc~` present: the patch reads the host's input.
    pub takes_audio: bool,
    /// `dac~` present: the patch writes to the host's output.
    pub gives_audio: bool,
    /// Objects the patch uses, in the order they appear. Kept because "why
    /// will this patch not load" is otherwise unanswerable.
    pub objects: Vec<String>,
    /// The patch's on-screen controls: sliders, number boxes and toggles.
    ///
    /// **This is how a headless host plays a patch.** Pd's controls are part of
    /// the patch rather than of the objects around them, and a slider left at
    /// zero holds the whole patch at zero — which is what a gain slider does
    /// while nobody is looking at the canvas. choz has no canvas, so the ones
    /// that can be addressed become knobs and the ones that cannot are named
    /// out loud.
    pub controls: Vec<PatchControl>,
}

/// One control of a patch, as the file describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchControl {
    /// The label drawn next to it, or the object's name when it has none.
    pub name: String,
    /// The symbol it listens on. **`None` is the important case**: a control
    /// with no receive symbol cannot be moved by anything outside the patch,
    /// including this host — see [`PatchInfo::controls`].
    pub receive: Option<String>,
    pub min: f32,
    pub max: f32,
    /// On/off rather than a range.
    pub toggle: bool,
}

impl PatchInfo {
    /// What choz can do with this patch, if anything.
    ///
    /// **The bar is `adc~` and `dac~`**, both of them. A patch choz can host is
    /// one that takes the host's audio and gives it back — that is what a slot
    /// in an FX chain *is*, and a patch that only half connects leaves the
    /// other half of the slot doing nothing. Everything else about a patch is
    /// optional: sliders, MIDI, whatever it does inside.
    pub fn role(&self) -> PatchRole {
        match self.takes_audio && self.gives_audio {
            true => PatchRole::Effect,
            false => PatchRole::Unusable,
        }
    }
}

/// Where a patch belongs in choz. Both cases turn on `adc~` **and** `dac~`;
/// see [`PatchInfo::role`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchRole {
    /// Audio in, audio out: an entry in the FX chain.
    Effect,
    /// It does not connect to the host's audio at both ends, so there is
    /// nothing to plug it into. Said out loud rather than hidden: "my patch
    /// does not appear" is the worst failure.
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
            // The GUI objects, whose arguments say what they control and
            // whether anybody outside the patch can reach them.
            "hsl" | "vsl" | "nbx" | "tgl" | "hradio" | "vradio" => {
                let rest: Vec<&str> = words.collect();
                if let Some(control) = read_control(object, &rest) {
                    info.controls.push(control);
                }
            }
            _ => {}
        }
        info.objects.push(object.to_string());
    }
    Ok(info)
}

/// One GUI object's arguments, in Pd's own order.
///
/// ```text
/// hsl  w h min max log init send receive label …
/// vsl  w h min max log init send receive label …
/// nbx  size h min max log init send receive label …
/// tgl  size init send receive label …
/// hradio size new_old init number send receive label …
/// ```
///
/// `empty` is Pd's way of writing "no symbol", and a control whose receive is
/// `empty` is one this host cannot move — which is worth knowing before the
/// patch is played rather than after.
fn read_control(kind: &str, args: &[&str]) -> Option<PatchControl> {
    let at = |i: usize| args.get(i).copied().unwrap_or("empty");
    let symbol = |s: &str| match s {
        "empty" | "-" | "" => None,
        other => Some(other.trim_start_matches('\\').to_string()),
    };
    let number = |s: &str| s.parse::<f32>().ok();
    let (receive, label, min, max, toggle) = match kind {
        "hsl" | "vsl" | "nbx" => (
            symbol(at(7)),
            at(8).to_string(),
            number(at(2)).unwrap_or(0.0),
            number(at(3)).unwrap_or(1.0),
            false,
        ),
        "tgl" => (symbol(at(3)), at(4).to_string(), 0.0, 1.0, true),
        "hradio" | "vradio" => (
            symbol(at(5)),
            at(6).to_string(),
            0.0,
            number(at(3)).map(|n| (n - 1.0).max(1.0)).unwrap_or(7.0),
            false,
        ),
        _ => return None,
    };
    let name = match label.as_str() {
        "empty" | "-" | "" => kind.to_string(),
        other => other.replace('_', " "),
    };
    Some(PatchControl {
        name,
        receive,
        min,
        max,
        toggle,
    })
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

/// The controls a host can actually move, in the order it should show them.
///
/// **The order is the contract** between the two sides: the interface builds
/// its knobs from this list and the child maps a parameter index back to a
/// receive symbol with the same call, so index `n` is the same control in both
/// processes without either of them saying so out loud.
pub fn addressable(info: &PatchInfo) -> Vec<&PatchControl> {
    info.controls
        .iter()
        .filter(|c| c.receive.is_some())
        .collect()
}

/// The controls that exist on the canvas and **cannot be reached** — no receive
/// symbol, so nothing outside the patch can move them.
///
/// Worth naming rather than counting: a patch whose gain slider is one of these
/// is a patch that sits at whatever the slider was saved at, which for a fresh
/// `hsl` is **zero**. That is silence with no error anywhere, and it is the
/// single most likely reason a patch "does nothing" in a host with no canvas.
pub fn unreachable(info: &PatchInfo) -> Vec<&str> {
    info.controls
        .iter()
        .filter(|c| c.receive.is_none())
        .map(|c| c.name.as_str())
        .collect()
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

    /// Move one of the patch's controls, by its position in [`addressable`].
    ///
    /// `value` is 0..1, the way every parameter in choz travels; it is scaled
    /// to the control's own range here, where the range is known.
    #[cfg(feature = "pd")]
    pub fn set_control(&mut self, index: usize, value: f32) {
        let Some(control) = addressable(&self.info).get(index).copied() else {
            return;
        };
        let Some(receive) = control.receive.as_deref() else {
            return;
        };
        let v = control.min + value.clamp(0.0, 1.0) * (control.max - control.min);
        imp::send_float(receive, v);
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
        fn libpd_add_to_search_path(path: *const c_char);
        fn libpd_float(recv: *const c_char, x: f32) -> c_int;
        fn libpd_set_printhook(hook: extern "C" fn(*const c_char));
    }

    /// Where Pd's own abstractions live on a Linux box.
    ///
    /// **libpd is not Pure Data**: it carries no installation, so it starts
    /// with an empty search path and every object that Pd itself ships as an
    /// abstraction — `rev1~`, `rev2~`, `rev3~`, `hilbert~`, `complex-mod~`, all
    /// of `extra` — fails to create. The patch still opens, with a hole where
    /// that object was, and the effect does nothing. That is exactly how a
    /// reverb patch built around `rev2~` arrived here as "it does not work".
    const SYSTEM_EXTRA: &[&str] = &[
        "/usr/lib/puredata/extra",
        "/usr/lib/pd/extra",
        "/usr/local/lib/pd/extra",
    ];

    /// Everything Pd prints — including "couldn't create" — goes to choz's log.
    ///
    /// Without this, a missing object is silent: Pd says so on a console
    /// nobody is reading, and the host has no way to know the patch came up
    /// half built.
    extern "C" fn print_out(text: *const c_char) {
        if text.is_null() {
            return;
        }
        let msg = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
        let msg = msg.trim_end();
        if !msg.is_empty() {
            eprintln!("pd: {msg}");
        }
    }

    static STARTED: AtomicBool = AtomicBool::new(false);

    /// Send a float to a receive symbol, which is how a host moves a control
    /// that has one.
    pub(super) fn send_float(receive: &str, value: f32) {
        if let Ok(name) = CString::new(receive) {
            unsafe { libpd_float(name.as_ptr(), value) };
        }
    }

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
            // **The print hook goes on before `libpd_init`**: Pd prints its
            // banner and its first complaints during init, and a hook set
            // afterwards misses exactly the messages that say why a patch came
            // up half built.
            libpd_set_printhook(print_out);
            libpd_init();
            // Where to look for objects the patch names: the patch's own
            // folder first (that is where a project keeps its abstractions),
            // then an `externals` beside it, then Pd's own `extra`.
            let mut search: Vec<String> = vec![dir.to_string_lossy().into_owned()];
            search.push(dir.join("externals").to_string_lossy().into_owned());
            search.extend(SYSTEM_EXTRA.iter().map(|p| p.to_string()));
            if let Some(env) = std::env::var_os("PD_PATH") {
                search.extend(
                    std::env::split_paths(&env).map(|p| p.to_string_lossy().into_owned()),
                );
            }
            for path in search {
                if let Ok(c) = CString::new(path) {
                    libpd_add_to_search_path(c.as_ptr());
                }
            }
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
        assert_eq!(info.role(), PatchRole::Effect);
        assert!(info.objects.iter().any(|o| o == "*~"), "{:?}", info.objects);

        // Notes are neither here nor there: what decides whether choz can host
        // a patch is the audio pair, and a patch may do whatever else it likes.
        let algo = write(
            &dir,
            "arp.pd",
            "#N canvas 0 0 450 300 12;\n\
             #X obj 20 20 adc~;\n\
             #X obj 20 40 dac~;\n\
             #X obj 20 60 notein;\n\
             #X obj 20 80 noteout;\n",
        );
        let info = read_patch(&algo).unwrap();
        assert_eq!(info.role(), PatchRole::Effect);

        // Notes and nothing else is **not** hostable: there is no slot shape
        // for a patch that neither takes the audio nor gives any back.
        let notes_only = write(
            &dir,
            "notes-only.pd",
            "#N canvas 0 0 450 300 12;\n#X obj 20 20 notein;\n#X obj 20 60 noteout;\n",
        );
        assert_eq!(read_patch(&notes_only).unwrap().role(), PatchRole::Unusable);

        // Half connected is not connected: audio in with nowhere to go.
        let half = write(
            &dir,
            "half.pd",
            "#N canvas 0 0 450 300 12;\n#X obj 20 20 adc~;\n#X obj 20 60 *~ 0.5;\n",
        );
        assert_eq!(read_patch(&half).unwrap().role(), PatchRole::Unusable);

        // Neither: there is nothing to wire it to, and saying so beats hiding it.
        let dud = write(
            &dir,
            "gui.pd",
            "#N canvas 0 0 450 300 12;\n#X obj 20 20 bng 15 250 50 0;\n",
        );
        assert_eq!(read_patch(&dud).unwrap().role(), PatchRole::Unusable);

        // And the directory scan finds them all, by name.
        let found = scan_directory(&dir);
        assert_eq!(found.len(), 5, "{found:?}");
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

    /// The real thing, end to end: a patch loaded into libpd, a block through
    /// it, and the number that comes out. Only with the feature on and Pure
    /// Data installed — everywhere else the crate still builds and the tests
    /// above cover what it says instead.
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
    /// A patch's on-screen controls, read from the file, with the one thing a
    /// headless host has to know about each: **whether it can be moved**.
    ///
    /// Both halves of this came from two real patches. A slider with no receive
    /// symbol —  Pd's default when you drop one on a canvas — cannot be
    /// addressed from outside, and a gain slider like that holds the whole
    /// patch at zero: silence, with no error anywhere, which is exactly how it
    /// was reported.
    #[test]
    fn a_patch_says_which_controls_a_host_can_move() {
        let dir = std::env::temp_dir().join("choz-pd-controls");
        std::fs::create_dir_all(&dir).unwrap();
        let path = write(
            &dir,
            "sliders.pd",
            "#N canvas 0 0 450 300 12;\n\
             #X obj 148 32 adc~ 1;\n\
             #X obj 152 280 dac~;\n\
             #X obj 216 52 hsl 170 20 0 10 0 0 empty gain GAIN -2 -10 0 12;\n\
             #X obj 365 142 hsl 170 20 0 1 0 0 empty empty Room -2 -10 0 12;\n\
             #X obj 100 200 tgl 19 0 empty bypass Bypass 17 7 0 10;\n",
        );
        let info = read_patch(&path).unwrap();
        assert_eq!(info.role(), PatchRole::Effect);
        assert_eq!(info.controls.len(), 3);

        // The two with a receive symbol are the knobs a host gets, in order.
        let usable = addressable(&info);
        assert_eq!(usable.len(), 2);
        assert_eq!(usable[0].name, "GAIN");
        assert_eq!(usable[0].receive.as_deref(), Some("gain"));
        assert_eq!((usable[0].min, usable[0].max), (0.0, 10.0));
        assert!(!usable[0].toggle);
        assert_eq!(usable[1].name, "Bypass");
        assert!(usable[1].toggle, "a toggle is two positions, not a range");

        // And the one without is named, because "my patch does nothing" is
        // answered by that name and by nothing else.
        assert_eq!(unreachable(&info), vec!["Room"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
