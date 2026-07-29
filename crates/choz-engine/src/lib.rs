//! choz audio engine: RT audio thread, sources, FX chain, MIDI input, and the
//! plugin registry. Built on the RT-safe traits in `choz-ports`.

pub mod fx;
pub mod fx_chain;
pub mod engine;
pub mod sources;
pub mod input;
pub mod midi;
pub mod osc;
pub mod cache;
pub mod paths;
pub mod registry;
pub mod scanner;
pub mod plugin_types;

pub use engine::AudioEngine;

/// Parameter index that means "choz's own dry/wet" in
/// [`AudioEngine::set_fx_param`], rather than one of the processor's params.
pub const FX_MIX_PARAM: usize = usize::MAX;
pub use fx_chain::FxSpec;
pub use registry::PluginRegistry;
pub use plugin_types::{PluginDescriptor, PluginKind};
pub use paths::{FoundPlugin, PluginFormat, PluginPaths, SearchDir};

/// Scan every enabled directory of every format in `paths`. CLAP entries get
/// real descriptor metadata (name/id/instrument flag) when this build hosts
/// CLAP; everything else is identified by file name.
pub fn scan_all(paths: &PluginPaths) -> Vec<FoundPlugin> {
    let mut out = Vec::new();
    for (format, dirs) in paths.entries.iter() {
        for dir in dirs.iter().filter(|d| d.enabled) {
            if *format == PluginFormat::Clap {
                out.extend(choz_plugin_clap::scan_directory(&dir.path).into_iter().map(|p| {
                    FoundPlugin {
                        format: PluginFormat::Clap,
                        name: p.name,
                        path: p.path,
                        id: p.id,
                        is_instrument: p.is_instrument,
                    }
                }));
            } else {
                out.extend(paths::scan_dir(&dir.path, *format));
            }
        }
    }
    out.sort_by_key(|p| (p.format, p.name.to_lowercase()));
    out.dedup_by(|a, b| a.path == b.path && a.id == b.id);
    out
}
pub use choz_plugin_clap::{ClapParamInfo, ClapPluginInfo};

/// Parameters exposed by a CLAP plugin. Non-RT (loads the plugin).
pub fn read_clap_params(path: &std::path::Path, plugin_id: &str) -> Vec<ClapParamInfo> {
    choz_plugin_clap::read_params(path, plugin_id)
}
