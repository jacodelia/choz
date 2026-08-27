//! What kind of control a parameter deserves.
//!
//! In the engine rather than in the interface because the two note generators
//! live here now — [`crate::arp`] describes its knobs with this, and the CLAP
//! bundle carries them out of choz. The interface re-exports it under its old
//! name, so every `source::ParamShape` still reads the same.

/// The control a parameter deserves, decided by what the parameter *is*.
///
/// Never guessed from the name — that is the mistake `FxCategory::guess` gets
/// away with because a wrong category only misfiles a row in a list, while a
/// cutoff drawn as a switch is unusable. A host that reports nothing leaves
/// everything [`ParamShape::Continuous`], which is what choz always drew.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ParamShape {
    /// A knob: any value in the range.
    #[default]
    Continuous,
    /// On or off. Nothing in between exists, so an arc at 0.00 is a lie.
    Toggle,
    /// Named positions in order, each with the 0..1 place it sits at.
    ///
    /// The positions are **not** evenly spaced in general: Ardour's a-delay
    /// names ten note divisions at 1, 2, 4, 6, 8, 12, 16, 24, 32 and 48 over a
    /// range of 1..48. Assuming a uniform grid there shows the wrong name and
    /// steps to values the plugin never offered.
    Named(Vec<(f32, String)>),
    /// A travel rather than a rotation: a mix, a pan, a time. Same values a
    /// knob would take, drawn as the distance covered because that is how the
    /// parameter reads.
    ///
    /// Carries the plugin's unit, which is both why it is a fader and how a run
    /// of them is recognised as one group — an ADSR is four consecutive times.
    Fader(String),
}

/// Units that mean "a distance along something" — a time, a share, a position.
///
/// This is the plugin's own `units:unit`, not a guess at its name: `FxCategory`
/// guesses from names and gets away with it because a wrong category only
/// misfiles a row, while a control that does not match the parameter is used
/// wrong. A plugin that reports no unit keeps the knob.
/// `pc` is LV2's name for percent (`units:pc`), and after inline definitions it
/// and `ms` are the two most common units in the 261 bundles installed here.
const FADER_UNITS: &[&str] = &[
    "s", "ms", "sec", "seconds", "%", "pc", "percent", "cent", "cents",
    // Not measurements: tags a plugin's own list uses to say "these belong
    // together and their shape is the point" — a set of harmonics, and the
    // phase of each. Drawn as a bank of bars, which is the only way 32 of them
    // read as a spectrum rather than as 32 numbers.
    "harmonic", "phase",
];

impl ParamShape {
    /// The shape a hosted plugin's parameter reports.
    pub fn of(p: &crate::PluginParam) -> Self {
        if p.is_toggle() {
            return ParamShape::Toggle;
        }
        // Named steps only when every step has a name: a partial list would
        // draw "3/8" for the ones the plugin skipped and lie about the rest.
        if !p.points.is_empty() && p.points.len() as u32 == p.steps {
            return ParamShape::Named(
                p.points
                    .iter()
                    .map(|(v, l)| (p.normalised(*v) as f32, l.clone()))
                    .collect(),
            );
        }
        // A time, a share or a position is read as how far along it is; a
        // frequency or a gain is read as a setting. The unit is the only thing
        // the plugin says about which of the two this is.
        if p.unit.as_deref().is_some_and(|u| {
            let u = u.trim().to_lowercase();
            FADER_UNITS.contains(&u.as_str())
        }) {
            return ParamShape::Fader(p.unit.clone().unwrap_or_default());
        }
        ParamShape::Continuous
    }

    /// Where one press of `←`/`→` lands.
    ///
    /// A stepped parameter moves one position, not one twentieth of its range:
    /// a switch nudged by 0.05 needs twenty presses to flip and spends the
    /// other nineteen in places it has no name for.
    pub fn nudge(&self, current: f32, delta: f32) -> f32 {
        let Some((k, n)) = self.step_at(current) else {
            return (current + delta).clamp(0.0, 1.0);
        };
        let dir: i64 = if delta >= 0.0 { 1 } else { -1 };
        let next = (k as i64 + dir).clamp(0, n as i64 - 1) as usize;
        self.position_of(next)
    }

    /// The 0..1 value of step `k`.
    fn position_of(&self, k: usize) -> f32 {
        match self {
            ParamShape::Continuous | ParamShape::Fader(_) => 0.0,
            ParamShape::Toggle => k.min(1) as f32,
            ParamShape::Named(points) => points.get(k).map(|(v, _)| *v).unwrap_or(0.0),
        }
    }

    /// Index of the position `norm` (0..1) selects, and how many there are.
    /// `None` for a continuous parameter, which has neither.
    pub fn step_at(&self, norm: f32) -> Option<(usize, usize)> {
        let norm = norm.clamp(0.0, 1.0);
        match self {
            // Neither has positions: they take any value in the range.
            ParamShape::Continuous | ParamShape::Fader(_) => None,
            ParamShape::Toggle => Some((usize::from(norm >= 0.5), 2)),
            ParamShape::Named(points) if points.is_empty() => None,
            // Nearest, not rounded onto a grid: the positions can sit anywhere.
            ParamShape::Named(points) => {
                let k = points
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a.0 - norm).abs().total_cmp(&(b.0 - norm).abs()))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                Some((k, points.len()))
            }
        }
    }

    /// The label of step `k`, when it has one.
    pub fn label(&self, k: usize) -> Option<&str> {
        match self {
            ParamShape::Named(points) => points.get(k).map(|(_, l)| l.as_str()),
            _ => None,
        }
    }
}
