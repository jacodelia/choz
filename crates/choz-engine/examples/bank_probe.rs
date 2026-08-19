//! Does a real Surge .fxp actually load into the VST3 instance?
fn main(){
    if choz_engine::worker_main(){return;}
    let dir = std::path::Path::new("/usr/share/surge-xt/patches_factory");
    let bank = choz_engine::preset_files::list_bank(dir);
    println!("bank: {} presets, first = {:?}", bank.len(), bank.first());
    let i = choz_engine::engine::build_instrument(choz_engine::PluginFormat::Vst3, std::path::Path::new("/usr/lib/vst3/Surge XT.vst3"), "", 48_000, 128).unwrap();
    let st = i.state().unwrap();
    let before = st.save().unwrap();
    let entry = bank.iter().find(|e| e.name == "Tok").or(bank.first()).unwrap();
    let blob = choz_engine::preset_files::read_state(std::path::Path::new(&entry.key)).unwrap();
    println!("patch {:?}: {} bytes, head {:?}", entry.name, blob.len(), String::from_utf8_lossy(&blob[..4]));
    st.restore(&blob);
    let after = st.save().unwrap();
    println!("state {} -> {} bytes, changed = {}", before.len(), after.len(), before != after);
    // And it has to make sound.
    let mut i = i;
    i.note_on(60, 100);
    let mut buf = vec![0.0f32; 256];
    let mut peak = 0.0f32;
    for _ in 0..200 { i.render(&mut buf, 48_000); peak = peak.max(buf.iter().fold(0.0f32,|a,s| a.max(s.abs()))); }
    println!("peak after loading the patch = {peak:.3}");
}
