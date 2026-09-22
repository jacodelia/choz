#!/usr/bin/env python3
"""The arranger's rhythms -> its style table, as Rust.

    tools/mid_to_styles.py crates/choz-engine/src/artifacts/arranger/rhythms \
        crates/choz-engine/src/artifacts/arranger/styles.rs
    cargo fmt -p choz-engine

A rhythm is one type-1 MIDI file holding a whole accompaniment: its sections
laid end to end, each opening with a marker naming the section, its bar count
and its time signature (`Var1 4bar 4/4`), and every track named with the
section and the part that plays it (`Var1 Bass`, `Var1 Drum`, `Var1 Chord2`).

A style is the *measurement* of one of those rhythms — how dense the bass is,
where the kick lands, what the comp holds — and not the notes themselves,
because the generators in `generate.rs` are still what plays.

Everything here is read off the file. Where a number cannot be measured from a
few bars over one chord (how often a bass walks chromatically into the next
chord, say) the comment on the line says so.
"""
import glob, os, re, struct, sys

PPQ_ASSUMED = 96
KICK, SNARE = {35, 36}, {37, 38, 39, 40}
HIHAT, RIDE = {42, 44, 46}, {51, 53, 59}


# ------------------------------------------------------------------ MIDI input

def vlq(b, p):
    n = 0
    while True:
        c = b[p]
        p += 1
        n = (n << 7) | (c & 0x7F)
        if not c & 0x80:
            return n, p


def read_midi(path):
    """-> (ppq, tempo_bpm, [(name, [(tick, note, vel, dur)], program)], [(tick, marker)])"""
    b = open(path, "rb").read()
    assert b[:4] == b"MThd"
    _, ntr, ppq = struct.unpack_from(">HHH", b, 8)
    p = 8 + struct.unpack_from(">I", b, 4)[0]
    tracks, markers, bpm = [], [], 120.0
    for _ in range(ntr):
        assert b[p:p + 4] == b"MTrk"
        ln = struct.unpack_from(">I", b, p + 4)[0]
        q, end, t, status = p + 8, p + 8 + ln, 0, 0
        name, on, notes, program = "", {}, [], None
        while q < end:
            dt, q = vlq(b, q)
            t += dt
            if b[q] & 0x80:
                status = b[q]
                q += 1
            if status == 0xFF:
                kind = b[q]
                n, q = vlq(b, q + 1)
                if kind == 0x03:
                    name = b[q:q + n].decode("latin1")
                elif kind == 0x06:
                    markers.append((t, b[q:q + n].decode("latin1")))
                elif kind == 0x51:
                    bpm = 60_000_000 / int.from_bytes(b[q:q + n], "big")
                q += n
            elif status & 0xF0 in (0x80, 0x90):
                note, vel = b[q], b[q + 1]
                q += 2
                if status & 0xF0 == 0x90 and vel:
                    # A second hit on a sounding note ends the first. Drum
                    # one-shots are written without note-offs at all, so
                    # without this every repeat of a kick would overwrite the
                    # one before it and the pattern would read as one hit.
                    if note in on:
                        start, v = on.pop(note)
                        notes.append((start, note, v, max(t - start, 1)))
                    on[note] = (t, vel)
                elif note in on:
                    start, v = on.pop(note)
                    notes.append((start, note, v, max(t - start, 1)))
            elif status & 0xF0 in (0xA0, 0xB0, 0xE0):
                q += 2
            else:
                if status & 0xF0 == 0xC0 and program is None:
                    program = b[q]
                q += 1
        # Drum hits are one-shots: the rhythm never writes a note-off for them,
        # so what is still held at the end of the track is a hit, not a leak.
        notes += [(start, note, v, 1) for note, (start, v) in on.items()]
        tracks.append((name, sorted(notes), program))
        p = end
    return ppq, bpm, tracks, markers


# ------------------------------------------------------------- measuring a bar

def median(xs, default=0.0):
    xs = sorted(xs)
    return xs[len(xs) // 2] if xs else default


def fold(notes, at, beats, ppq, keep=None):
    """Note onsets inside one element, folded onto a single bar, in beats.
    Positions are exact — [`snap`] is what rounds them, and swing has to be
    measured before any rounding or the shuffle is rounded away."""
    return [((tick - at) / ppq % beats, note, vel, dur / ppq)
            for tick, note, vel, dur in notes
            if tick >= at and (not keep or note in keep)]


def snap(pos, triplets=False):
    """To the thirty-second grid, and to the triplet grid as well when the
    style was measured as swung: a shuffle lives at 2/3 of the beat and
    rounding it to 0.625 is what turns a shuffle into a sixteenth. A style that
    came out straight does not get the triplet grid at all — there, a position
    a hair off the beat is a push and not a triplet, and 1/12 of a beat
    is a gap no kit plays."""
    straight = round(pos * 8) / 8
    if not triplets:
        return round(straight, 4)
    triplet = round(pos * 12) / 12
    return round(straight if abs(pos - straight) <= abs(pos - triplet) else triplet, 4)


def positions(folded, bars, triplets, share=0.34):
    """The onsets a listener would call part of the pattern: the ones that come
    back in more than a third of the bars."""
    return sorted(p for p, n in counts(folded, triplets).items()
                  if n >= max(1, round(bars * share)))


def counts(folded, triplets):
    out = {}
    for pos, *_ in folded:
        out[snap(pos, triplets)] = out.get(snap(pos, triplets), 0) + 1
    return out


def register(lo, hi, span, floor, ceil):
    """The measured notes, widened to a register the generator can work in.

    What a rhythm plays is a sample: four bars over one chord in C. A bass that
    happened to sit on one note there still has a bass's range, and a voicing
    cannot be led anywhere inside six semitones — so the measured span is the
    centre of the register and not the whole of it."""
    lo, hi = max(lo, floor), min(hi, ceil)
    short = max(span - (hi - lo), 0) / 2
    lo, hi = round(lo - short), round(hi + short)
    lo, hi = max(lo, floor), min(hi, ceil)
    return (lo, max(hi, lo + 1))


def in_bar(ps, beats):
    """Drops the position a note a hair before the bar line snaps up to."""
    return [p for p in ps if p < beats - 1e-9]


def swing_of(folded, div):
    """How late the off-divisions sit, as the share of `div` the generator adds
    on top of them. 0 is straight, 1/3 is a triplet shuffle.

    Only onsets in the second half of the division's cell count, and only the
    ones that are not already on the next straight subdivision: a pattern in
    straight sixteenths has an onset at 0.75 of the beat, and reading that as
    "an eighth played half a division late" would make every sixteenth pattern
    a shuffle."""
    cell = [pos % (2 * div) for pos, *_ in folded]
    if not cell:
        return 0.0
    # If the pattern plays the *first* half-division as well, it is subdividing
    # rather than swinging, and the onset at three quarters of the cell is a
    # sixteenth and not a very late eighth.
    early = sum(1 for x in cell if abs(x - div / 2) < 0.12 * div)
    if early >= 0.12 * len(cell):
        return 0.0
    late = [(x - div) / div for x in cell if 0.05 < (x - div) / div < 0.75]
    return round(median(late), 3) if len(late) >= 4 else 0.0


# ------------------------------------------------------------------- the style

def slug(rel):
    """`pop/017 16BtShfl.mid` -> `16btshfl`. The folder and any leading number
    go: the number is a slot in whatever the rhythm was exported from, and the
    name is what somebody types into a chart."""
    base = os.path.splitext(os.path.basename(rel))[0]
    base = base.split(" ", 1)[-1] if base[:1].isdigit() else base
    out = "".join(c.lower() if c.isalnum() else "_" for c in base)
    while "__" in out:
        out = out.replace("__", "_")
    return out.strip("_")


def style_of(path, rel):
    ppq, bpm, tracks, markers = read_midi(path)
    # The element to measure is the one a rhythm spends its time in.
    els = {name.split()[0]: (at, name) for at, name in markers}
    order = [e for e in ("Var1", "Var2", "Intro") if e in els]
    if not order:
        return None
    at, label = els[order[0]]
    bars, sig = int(label.split()[1].rstrip("bar")), label.split()[2]
    num, den = (int(x) for x in sig.split("/"))
    beats = num * 4 / den
    has_fill = any(n.startswith("Fill") for _, n in markers)

    def gm(part_name, *fallback):
        """The rhythm's own patch first, then what any SoundFont has. The bank
        a rhythm names may be its own, so only the program number carries over —
        and a program that is not in the bank is why there is a fallback at
        all. See `Style::programs`."""
        p = programs.get(part_name)
        return ([p] if p is not None and p not in fallback else []) + list(fallback)

    def part(*want):
        """Every note of the wanted parts inside the measured element.

        One track per part and not all of them: a part often holds a
        major and a minor take of the same bar, and counting both would read as
        twice the density it plays at."""
        best, prog = {}, {}
        for name, notes, program in tracks:
            fields = name.split()
            if len(fields) < 2 or fields[0] != order[0] or fields[1] not in want:
                continue
            n = [x for x in notes if at <= x[0] < at + bars * beats * ppq]
            if len(n) >= len(best.get(fields[1], [])):
                best[fields[1]], prog[fields[1]] = n, program
        programs[want[0]] = next((p for p in prog.values() if p is not None), None)
        return sorted(x for n in best.values() for x in n)

    programs = {}
    drum = part("Drum", "Percussion")
    bass = part("Bass")
    comp = part("Chord1", "Chord2", "Chord3")

    d = fold(drum, at, beats, ppq)
    eighths, sixteenths = swing_of(d, 0.5), swing_of(d, 0.25)
    swing, swing_div = (eighths, 0.5) if eighths else (sixteenths, 0.25 if sixteenths else 0.5)

    loud = [x for x in fold(drum, at, beats, ppq, SNARE) if x[2] >= 55]
    tri = swing > 0.0
    kick = positions(fold(drum, at, beats, ppq, KICK), bars, tri) or [0.0]
    # The backbeat only: the quiet snares are the ghosts, and they are a rate
    # rather than a position — see `ghost` below.
    snare = positions(loud, bars, tri)
    cym = sorted({snap(p, tri) for p, *_ in fold(drum, at, beats, ppq, HIHAT | RIDE)})
    gaps = [round(b - a, 4) for a, b in zip(cym, cym[1:]) if 0.1 <= b - a <= 2]
    hats = fold(drum, at, beats, ppq, HIHAT)
    rides = fold(drum, at, beats, ppq, RIDE)
    quiet = [1 for p, n, v, _ in fold(drum, at, beats, ppq, SNARE) if v < 55]

    bf = fold(bass, at, beats, ppq)
    pitches = [n for _, n, _, _ in bass] or [36]
    steps = [b - a for a, b in zip(pitches, pitches[1:])]
    cf = fold(comp, at, beats, ppq)
    cpitch = [n for _, n, _, _ in comp] or [60]

    return {
        "name": slug(rel),
        "tempo": round(bpm),
        "beats_per_bar": round(beats, 3),
        # The signature as written, which `beats_per_bar` cannot say: 6/8 and
        # 3/4 are both three quarter notes and are not the same bar.
        "meter": (num, den),
        "swing": swing,
        "swing_div": swing_div,
        # The kit's loud core. A median over every hit would be dragged down by
        # the hi-hat, which is quiet by design and is not how hard a style hits.
        "vel": max(1, min(127, round(median(
            [v for _, n, v, _ in drum if n in KICK | SNARE] or
            [v for _, _, v, _ in drum or bass], 96)))),
        # Rhythm libraries are written on the grid, so there is no looseness to
        # measure: this is the generator's own, the same for every style.
        "human": 0.6,
        "strum": 0.03,
        "bass": {
            "walking": len(bf) >= beats * 0.9 and len(set(pitches)) >= 3,
            "density": round(min(len(bf) / max(bars, 1) / beats, 1.0), 3),
            "approach": round(sum(1 for s in steps if abs(s) == 1) / max(len(steps), 1), 3),
            "passing": round(sum(1 for s in steps if abs(s) == 2) / max(len(steps), 1), 3),
            "pickup": round(sum(1 for p, *_ in bf if p >= beats - 0.5) / max(bars, 1), 3),
            "octave_jump": round(sum(1 for s in steps if abs(s) >= 11) / max(len(steps), 1), 3),
            "low": register(min(pitches), max(pitches), 16, 28, 60)[0],
            "high": register(min(pitches), max(pitches), 16, 28, 60)[1],
        },
        "drums": {
            "kick": in_bar(kick, beats) or [0.0], "snare": in_bar(snare, beats),
            "cymbal": median(gaps) if gaps else 0.0,
            "ride": len(rides) > len(hats),
            # Quiet snares per bar, against the off-eighths there was room for.
            "ghost": round(min(len(quiet) / max(bars, 1) / max(beats * 2, 1), 1.0), 3),
            "fill_every": 4 if has_fill else 0,
        },
        "comp": {
            "hits": in_bar(positions(cf, bars, tri), beats) or [0.0],
            # How often a hit of the pattern is actually there: a comp that
            # plays all four of its hits in every bar is 1, one that leaves
            # half of them out somewhere is around 0.5.
            "density": round(min(median(
                [min(counts(cf, tri).get(p, 0) / max(bars, 1), 1.0)
                 for p in positions(cf, bars, tri)], 1.0), 1.0), 3),
            # Clamped to the bar: a pad that lies under four bars is still a
            # comp hit the generator has to let go of when the chord turns.
            "hold": round(min(median([d / ppq for _, _, _, d in comp], 0.5), beats), 3),
            "low": register(min(cpitch), max(cpitch), 24, 36, 88)[0],
            "high": register(min(cpitch), max(cpitch), 24, 36, 88)[1],
        },
        "bass_gm": gm("Bass", 33, 32, 0),
        "piano_gm": gm("Chord1", 4, 0),
        "guitar_gm": gm("Chord3", 27, 24, 0),
    }


# ---------------------------------------------------------------- style names

# A rhythm file is named the way an instrument's front panel had room for it —
# `16btshfl`, `pnrckbld` — and a list of two hundred of those is not something
# anybody can read. The slug stays the name a chart writes; this is what the
# picker shows. Longest match first, and **only expansions that are certain**:
# a rhythm whose abbreviation could be two things keeps it, because a confident
# wrong name is worse than a short one.
WORDS = [
    ("unplgbld", "unplugged ballad"), ("strqrtet", "string quartet"),
    ("strd_pno", "stride piano"), ("fstbband", "fast big band"),
    ("midbband", "mid big band"), ("slwbband", "slow big band"),
    ("amrcnrck", "american rock"), ("argcmbia", "argentine cumbia"),
    ("teccmbia", "techno cumbia"), ("grmnmrch", "german march"),
    ("pnrckbld", "piano rock ballad"), ("pianor_r", "piano rock & roll"),
    ("n_o_r_r", "new orleans rock & roll"), ("f_gospel", "fast gospel"),
    ("s_gospel", "slow gospel"), ("e_hiphop", "electro hip hop"),
    ("fingcnty", "fingerpicking country"), ("quickstp", "quickstep"),
    ("pasodble", "paso doble"), ("bosanova", "bossa nova"),
    ("slwbossa", "slow bossa"), ("chinspop", "chinese pop"),
    ("bluegras", "bluegrass"), ("valenato", "vallenato"),
    ("guangdon", "guangdong"), ("vienwltz", "viennese waltz"),
    ("popregae", "pop reggae"), ("regeton", "reggaeton "),
    ("melow8bt", "mellow 8 beat"), ("oldie8bt", "oldies 8 beat"),
    ("orgnrock", "organ rock"), ("strtrock", "straight rock"),
    ("strt_8bt", "straight 8 beat"), ("pno8beat", "piano 8 beat"),
    ("ochswing", "orchestral swing"), ("arpegio", "arpeggio "),
    ("xmassong", "christmas song"), ("xmaswltz", "christmas waltz"),
    ("fox_trot", "foxtrot"), ("euro_pop", "euro pop"),
    ("elec_bld", "electric ballad"), ("easy_bld", "easy ballad"),
    ("a_gt_pop", "acoustic guitar pop"), ("tech_pop", "techno pop"),
    ("synthpop", "synth pop"), ("discopop", "disco pop"),
    ("dscosoul", "disco soul"), ("fastsoul", "fast soul"),
    ("slowsoul", "slow soul"), ("old_soul", "old soul"),
    ("old_rock", "old rock"), ("slowrock", "slow rock"),
    ("rckblues", "rock blues"), ("shfblues", "shuffle blues"),
    ("slwblues", "slow blues"), ("bluesbld", "blues ballad"),
    ("brushbld", "brush ballad"), ("shflrock", "shuffle rock"),
    ("ltinrock", "latin rock"), ("ltinfusn", "latin fusion"),
    ("rockwltz", "rock waltz"), ("cntywltz", "country waltz"),
    ("pnowaltz", "piano waltz"), ("polwaltz", "polka waltz"),
    ("fr_waltz", "french waltz"), ("jz_waltz", "jazz waltz"),
    ("slwwaltz", "slow waltz"), ("slwswing", "slow swing"),
    ("funkshfl", "funk shuffle"), ("funk16bt", "funk 16 beat"),
    ("funk_8bt", "funk 8 beat"), ("gtr_8bt", "guitar 8 beat"),
    ("cnty_8bt", "country 8 beat"), ("cnty_bld", "country ballad"),
    ("cnty_pop", "country pop"), ("cntyshfl", "country shuffle"),
    ("mdrncnty", "modern country"), ("mdrn_bld", "modern ballad"),
    ("mdrn_r_b", "modern r&b"), ("pop_shfl", "pop shuffle"),
    ("pop_rock", "pop rock"), ("pop_bld", "pop ballad"),
    ("rock_bld", "rock ballad"), ("slow_bld", "slow ballad"),
    ("slowbld", "slow ballad "), ("pnobld", "piano ballad "),
    ("pnomrch", "piano march "), ("jzcombo", "jazz combo "),
    ("6_8blues", "6/8 blues"), ("6_8rkbld", "6/8 rock ballad"),
    ("6_8bld", "6/8 ballad "), ("6_8_bld", "6/8 ballad"),
    ("6_8_pop", "6/8 pop"), ("6_8_enka", "6/8 enka"),
    ("16btshfl", "16 beat shuffle"), ("16bt_bld", "16 beat ballad"),
    ("16_beat", "16 beat"), ("60_sshfl", "60s shuffle"),
    ("60_ssoul", "60s soul"), ("60_srock", "60s rock"),
    ("60_s_8bt", "60s 8 beat"), ("60_s_pop", "60s pop"),
    ("50_srock", "50s rock"), ("50spnrck", "50s piano rock"),
    ("90_s_bld", "90s ballad"), ("hip_hop", "hip hop "),
    ("ep_bld", "electric piano ballad "), ("r_b_bld", "r&b ballad"),
    ("waltz", "waltz "), ("cumbia", "cumbia "), ("salsa", "salsa "),
    ("samba", "samba "), ("shouka", "shouka "), ("indipop", "indian pop "),
    ("r_b", "r&b"), ("r_r", "rock & roll"),
]

# Words a title case would get wrong.
CASED = {"r&b": "R&B", "uk": "UK"}


def label(slug, dup=0):
    """What the style picker shows: `16btshfl` -> `16 Beat Shuffle`.

    `dup` is the number this script added for a second rhythm that came in
    under a name already taken — see the dedup in `main`. It is kept apart from
    the slug because a rhythm's own number is part of its name: `pnobld_1` is
    Piano Ballad 1 and there is a Piano Ballad 2 beside it."""
    s = slug[: -len(f"_{dup}")] if dup else slug
    tail = f" ({dup})" if dup else ""
    for short, long in WORDS:
        if short in s:
            s = s.replace(short, long)
            break
    s = re.sub(r"\s+", " ", s.replace("_", " ").strip()) + tail
    return " ".join(
        CASED.get(w, w if w[:1].isdigit() or "(" in w else w.capitalize())
        for w in s.split(" ")
    )


# -------------------------------------------------------------- Rust emission

def f(x):
    x = float(x)
    return f"{int(x)}.0" if x == int(x) else repr(round(x, 4))


def slice_(xs):
    return "&[" + ", ".join(f(x) for x in xs) + "]"


def emit(styles, out):
    w = ["// Generated by tools/mid_to_styles.py. Do not edit.",
         "//",
         "// Every number is measured off the rhythm of the same name — see the",
         "// script for what each measurement is.",
         "",
         "use super::style::{Bass, Comp, Drums, Style};",
         "",
         f"pub const ALL: &[Style] = &[",
         ]
    for s in styles:
        b, d, c = s["bass"], s["drums"], s["comp"]
        w.append(f"""    Style {{
        name: "{s['name']}",
        label: "{label(s['name'], s['dup'])}",
        beats_per_bar: {f(s['beats_per_bar'])},
        meter: ({s['meter'][0]}, {s['meter'][1]}),
        swing: {f(s['swing'])},
        swing_div: {f(s['swing_div'])},
        vel: {s['vel']},
        human: {f(s['human'])},
        strum: {f(s['strum'])},
        bass: Bass {{ walking: {str(b['walking']).lower()}, density: {f(b['density'])}, approach: {f(b['approach'])}, passing: {f(b['passing'])}, pickup: {f(b['pickup'])}, octave_jump: {f(b['octave_jump'])}, low: {b['low']}, high: {b['high']} }},
        drums: Drums {{ kick: {slice_(d['kick'])}, snare: {slice_(d['snare'])}, cymbal: {f(d['cymbal'])}, ride: {str(d['ride']).lower()}, ghost: {f(d['ghost'])}, fill_every: {d['fill_every']} }},
        comp: Comp {{ hits: {slice_(c['hits'])}, density: {f(c['density'])}, hold: {f(c['hold'])}, low: {c['low']}, high: {c['high']} }},
        guitar: None,
        bass_gm: &{list(s['bass_gm'])},
        piano_gm: &{list(s['piano_gm'])},
        guitar_gm: &{list(s['guitar_gm'])},
    }},""")
    w.append("];")
    w.append("")
    open(out, "w").write("\n".join(w))


def main(argv):
    if len(argv) != 3:
        print(__doc__)
        return 2
    root, out = argv[1], argv[2]
    styles, seen = [], {}
    for path in sorted(glob.glob(os.path.join(root, "**", "*.mid"), recursive=True)):
        s = style_of(path, os.path.relpath(path, root))
        if not s:
            print(f"skipped {path}: no section markers")
            continue
        # A library usually holds the same rhythm more than once. Two that
        # measure the same are one style; two that only share a name are two,
        # and the second gets a number.
        s["dup"] = 0
        body = {k: v for k, v in s.items() if k not in ("name", "dup")}
        if s["name"] in seen:
            if seen[s["name"]] == body:
                continue
            n = 2
            while f"{s['name']}_{n}" in seen:
                n += 1
            s["name"], s["dup"] = f"{s['name']}_{n}", n
        seen[s["name"]] = body
        styles.append(s)
    styles.sort(key=lambda s: s["name"])
    emit(styles, out)
    print(f"{len(styles)} styles -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
