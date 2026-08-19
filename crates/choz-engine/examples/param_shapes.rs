//! What choz ends up drawing for a plugin's parameters, after the two things
//! it now reads from the plugin itself: whether a parameter can be automated at
//! all, and whether its whole range only ever reads as two words.
fn main() {
    if choz_engine::worker_main() {
        return;
    }
    let path = std::env::args()
        .nth(1)
        .unwrap_or("/usr/lib/vst3/Surge XT.vst3".into());
    let ps = choz_engine::read_plugin_params(
        choz_engine::PluginFormat::Vst3,
        std::path::Path::new(&path),
        "",
    );
    let toggles = ps.iter().filter(|p| p.is_toggle()).count();
    println!("{} parameters, {toggles} of them switches", ps.len());
    for p in ps.iter().filter(|p| {
        let l = p.name.to_lowercase();
        l.contains("mute") || l.contains("poly") || l.contains("midi cc")
    }).take(8) {
        println!(
            "  {:<28} toggle {:<5} points {:?}",
            p.name,
            p.is_toggle(),
            p.points
        );
    }
}
