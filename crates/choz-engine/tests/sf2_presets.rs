//! Smoke test against a real SoundFont, when one is installed. Skipped
//! (passes trivially) on machines without /usr/share/sounds/sf2.

use std::path::Path;

#[test]
fn lists_general_midi_programs() {
    let path = Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
    if !path.exists() {
        eprintln!("no system SoundFont — skipping");
        return;
    }
    let presets = choz_engine::sources::list_sf2_presets(path).expect("parse GM soundfont");
    assert!(presets.len() > 100, "GM font has 128+ programs, got {}", presets.len());
    assert!(presets.windows(2).all(|w| (w[0].bank, w[0].preset) <= (w[1].bank, w[1].preset)));
    assert!(presets.iter().all(|p| p.name != "EOP"), "terminal record filtered");
    assert_eq!(presets[0].label().split(' ').next(), Some("000:000"));
}
