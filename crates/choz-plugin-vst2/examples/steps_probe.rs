//! What the installed VST2 plugins say the positions of their parameters are.
//!
//! `cargo run --release -p choz-plugin-vst2 --example steps_probe`

fn main() {
    let mut dirs: Vec<std::path::PathBuf> = vec!["/usr/lib/vst".into(), "/usr/lib/lxvst".into()];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(&home).join(".vst"));
        dirs.push(std::path::PathBuf::from(&home).join("repo"));
    }
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        for info in choz_plugin_vst2::scan_directory(dir) {
            let t = std::time::Instant::now();
            let params = choz_plugin_vst2::read_params(&info.path, &info.name);
            let took = t.elapsed();
            let listed: Vec<&choz_ports::PluginParam> =
                params.iter().filter(|p| p.steps > 0).collect();
            println!(
                "{:<24} {:>3} params, {} with named positions, read in {:.0} ms",
                info.name,
                params.len(),
                listed.len(),
                took.as_secs_f32() * 1000.0
            );
            for p in listed.iter().take(4) {
                let names: Vec<&str> = p.points.iter().map(|(_, n)| n.as_str()).collect();
                println!("    {:<20} {:?}", p.name, names);
            }
        }
    }
}
