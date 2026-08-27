//! Does a u-he `.h2p` preset load as a plugin state blob?
//!
//! u-he ships its patches as text files (`~/.u-he/<Plugin>/Presets/…/x.h2p`),
//! not as `.fxp` or `.vstpreset` — and its own state chunk is that same text.
fn main() {
    if choz_engine::worker_main() {
        return;
    }
    let mut a = std::env::args().skip(1);
    let fmt = match a.next().as_deref() {
        Some("vst2") => choz_engine::PluginFormat::Vst2,
        _ => choz_engine::PluginFormat::Vst3,
    };
    let path = a
        .next()
        .unwrap_or("/home/jorge/.vst3/u-he/TyrellN6.vst3".into());
    let preset = a.next().unwrap_or_else(|| {
        "/home/jorge/.u-he/TyrellN6/Presets/TyrellN6/04 Instruments/Bell Flower.h2p".into()
    });

    let mut i =
        choz_engine::engine::build_instrument(fmt, std::path::Path::new(&path), "", 48_000, 128)
            .expect("load");
    let st = i.state().expect("state handle");
    let before = st.save().unwrap_or_default();
    println!(
        "state {} bytes, head {:?}",
        before.len(),
        String::from_utf8_lossy(&before[..before.len().min(48)])
    );

    let blob = std::fs::read(&preset).expect("read preset");
    println!(
        "preset {} bytes, head {:?}",
        blob.len(),
        String::from_utf8_lossy(&blob[..24])
    );
    st.restore(&blob);
    let after = st.save().unwrap_or_default();
    println!(
        "after restore: {} bytes, changed = {}",
        after.len(),
        after != before
    );

    i.note_on(60, 100);
    let mut buf = vec![0.0f32; 256];
    let mut peak = 0.0f32;
    for _ in 0..400 {
        i.render(&mut buf, 48_000);
        peak = peak.max(buf.iter().fold(0.0f32, |a, s| a.max(s.abs())));
    }
    println!("peak = {peak:.3}");
}
