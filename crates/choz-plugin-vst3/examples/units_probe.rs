//! What the installed VST3 plugins say their sections are.
//!
//! `cargo run --release -p choz-plugin-vst3 --example units_probe`

fn main() {
    let mut found = choz_plugin_vst3::scan_directory(std::path::Path::new("/usr/lib/vst3"));
    if let Some(home) = std::env::var_os("HOME") {
        found.extend(choz_plugin_vst3::scan_directory(
            &std::path::PathBuf::from(home).join(".vst3"),
        ));
    }
    for info in &found {
        let params = choz_plugin_vst3::read_params(&info.path, &info.name);
        let named = params.iter().filter(|p| p.group.is_some()).count();
        let mut groups: Vec<&str> = params.iter().filter_map(|p| p.group.as_deref()).collect();
        groups.sort_unstable();
        groups.dedup();
        println!(
            "{:<28} {:>4} params, {named} in {} sections: {:?}",
            info.name,
            params.len(),
            groups.len(),
            groups.iter().take(6).collect::<Vec<_>>()
        );
    }
}
