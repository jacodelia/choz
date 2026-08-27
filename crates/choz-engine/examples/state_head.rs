fn main() {
    if choz_engine::worker_main() {
        return;
    }
    let i = choz_engine::engine::build_instrument(
        choz_engine::PluginFormat::Vst3,
        std::path::Path::new("/usr/lib/vst3/Surge XT.vst3"),
        "",
        48_000,
        128,
    )
    .unwrap();
    let s = i.state().unwrap().save().unwrap();
    println!("len {}", s.len());
    println!(
        "{}",
        s[..64]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("{:?}", String::from_utf8_lossy(&s[..64]));
}
