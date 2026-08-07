//! Manual check: does a real CLAP instrument act on the sustain pedal we send?
//! `cargo run -p choz-engine --example clap_pedal`

fn main() {
    use choz_ports::AudioSource;
    let path = std::path::Path::new("/usr/lib/clap/Surge XT.clap");
    let ids = choz_plugin_clap::scan_directory(path.parent().unwrap());
    let Some(id) = ids.iter().find(|p| p.path == path && p.is_instrument) else {
        println!("Surge XT not found as an instrument");
        return;
    };
    println!("plugin: {} ({})", id.name, id.id);
    let Some(mut inst) = choz_plugin_clap::host::ClapInstrument::build(path, &id.id, 48_000, 512) else {
        println!("build failed");
        return;
    };
    // Level of the *last* block only: a max over the whole window would just
    // report the tail of the previous phase.
    let peak = |i: &mut choz_plugin_clap::host::ClapInstrument, blocks: usize| {
        let mut buf = vec![0.0f32; 1024];
        for _ in 0..blocks {
            i.render(&mut buf, 48_000);
        }
        buf.iter().fold(0.0f32, |a, v| a.max(v.abs()))
    };
    inst.note_on(60, 100);
    println!("sounding:      {:.4}", peak(&mut inst, 20));
    inst.note_off(60);
    println!("after off:     {:.4}", peak(&mut inst, 200));

    inst.control_change(64, 127); // sustain down
    inst.note_on(60, 100);
    println!("sounding+ped:  {:.4}", peak(&mut inst, 20));
    inst.note_off(60);
    println!("off, pedal held: {:.4}", peak(&mut inst, 200));
    inst.control_change(64, 0);
    println!("pedal lifted:  {:.4}", peak(&mut inst, 200));
}
