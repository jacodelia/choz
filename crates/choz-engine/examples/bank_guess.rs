//! Where choz thinks each installed synth keeps its patches.
fn main() {
    if choz_engine::worker_main() {
        return;
    }
    for (name, path) in [
        ("Surge XT", "/usr/lib/vst3/Surge XT.vst3"),
        ("TyrellN6", "/home/jorge/.vst3/u-he/TyrellN6.vst3"),
        ("TyrellN6", "/home/jorge/.vst/u-he/TyrellN6.64.so"),
        ("TripleCheese", "/home/jorge/.vst3/u-he/TripleCheese.vst3"),
        (
            "Pianoteq 9",
            "/home/jorge/repo/Pianoteq 9/x86-64bit/Pianoteq 9.lv2",
        ),
    ] {
        let p = std::path::Path::new(path);
        match choz_engine::preset_files::guess_bank_dir(name, p) {
            Some(dir) => {
                let bank = choz_engine::preset_files::list_bank(&dir);
                let cats: Vec<&str> = {
                    let mut c: Vec<&str> = bank.iter().map(|e| e.category.as_str()).collect();
                    c.sort();
                    c.dedup();
                    c
                };
                println!(
                    "{name:<14} -> {}\n{:16}{} patches, categories: {:?}",
                    dir.display(),
                    "",
                    bank.len(),
                    &cats[..cats.len().min(6)]
                );
            }
            None => println!("{name:<14} -> (nothing found)"),
        }
    }
}
