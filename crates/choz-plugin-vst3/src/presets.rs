//! The plugin's programs, through `IUnitInfo`.
//!
//! VST3 does not have a preset call. It has *program lists*: `IUnitInfo`
//! describes named lists of programs, and a unit points at the list it uses.
//! Selecting one is not a method either — it is a **parameter**, the one whose
//! `ParameterInfo` carries `kIsProgramChange`. So loading a program means
//! setting that parameter, on both sides:
//!
//! * on the controller, so the plugin's own window shows the new program, and
//! * into the [`EditFeed`], so the change reaches the processor with the next
//!   block. Setting only the controller is the bug that made Surge XT's knobs
//!   move here without the sound following.

use choz_ports::PresetEntry;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

use crate::host::{EditFeed, SharedState};

/// A program list and the parameter that selects within it.
struct ProgramList {
    /// The parameter carrying `kIsProgramChange` for the unit that owns this
    /// list, and the number of steps it has.
    param: ParamID,
    steps: f64,
}

pub struct Vst3Presets {
    shared: SharedState,
    edits: EditFeed,
    /// `(entry, program index)`, resolved once at load time.
    programs: Vec<(PresetEntry, i32)>,
    list: Option<ProgramList>,
}

/// Everything the controller of a live instance says about its programs.
///
/// Runs on the UI thread, against the same cell the state handle uses.
pub fn scan(shared: &SharedState) -> (Vec<(PresetEntry, i32)>, Option<ParamID>, f64) {
    let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(cell) = guard.as_ref() else {
        return (Vec::new(), None, 0.0);
    };
    let Some(controller) = cell.controller() else {
        return (Vec::new(), None, 0.0);
    };
    // SAFETY: the controller is live while the cell is `Some`.
    let Some(units) = controller.cast::<IUnitInfo>() else {
        return (Vec::new(), None, 0.0);
    };

    // The program-change parameter is what actually switches programs. Without
    // one there is nothing to offer, however many lists the plugin describes.
    let mut param = None;
    let mut steps = 0.0;
    let count = unsafe { controller.getParameterCount() }.max(0);
    for i in 0..count {
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if unsafe { controller.getParameterInfo(i, &mut info) } != kResultOk {
            continue;
        }
        if info.flags & ParameterInfo_::ParameterFlags_::kIsProgramChange != 0 {
            param = Some(info.id);
            steps = info.stepCount.max(1) as f64;
            break;
        }
    }
    if param.is_none() {
        return (Vec::new(), None, 0.0);
    }

    let mut out = Vec::new();
    let lists = unsafe { units.getProgramListCount() }.max(0);
    for i in 0..lists {
        let mut info: ProgramListInfo = unsafe { std::mem::zeroed() };
        if unsafe { units.getProgramListInfo(i, &mut info) } != kResultOk {
            continue;
        }
        let category = crate::host::w_arr_to_string(&info.name);
        for program in 0..info.programCount.max(0) {
            let mut name: String128 = unsafe { std::mem::zeroed() };
            if unsafe { units.getProgramName(info.id, program, &mut name) } != kResultOk {
                continue;
            }
            let name = crate::host::w_arr_to_string(&name);
            out.push((
                PresetEntry {
                    name: if name.is_empty() {
                        format!("Program {}", program + 1)
                    } else {
                        name
                    },
                    category: category.clone(),
                    key: program.to_string(),
                },
                program,
            ));
        }
    }
    (out, param, steps)
}

impl Vst3Presets {
    pub fn new(shared: SharedState, edits: EditFeed) -> Option<Self> {
        let (programs, param, steps) = scan(&shared);
        if programs.is_empty() {
            return None;
        }
        let param = param?;
        Some(Self {
            shared,
            edits,
            programs,
            list: Some(ProgramList { param, steps }),
        })
    }
}

impl choz_ports::PluginPresets for Vst3Presets {
    fn list(&self) -> Vec<PresetEntry> {
        self.programs.iter().map(|(e, _)| e.clone()).collect()
    }

    fn load(&self, key: &str) {
        let Some(list) = self.list.as_ref() else {
            return;
        };
        let Some((_, program)) = self.programs.iter().find(|(e, _)| e.key == key) else {
            return;
        };
        // The program-change parameter is normalised over its own step count,
        // like every other VST3 parameter.
        let value = (*program as f64 / list.steps).clamp(0.0, 1.0);
        {
            let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(controller) = guard.as_ref().and_then(|c| c.controller()) {
                // SAFETY: live controller under the cell's mutex.
                unsafe { controller.setParamNormalized(list.param, value) };
            }
        }
        // …and the half that makes it audible.
        self.edits.push(list.param, value);
    }
}
