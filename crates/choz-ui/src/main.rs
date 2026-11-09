//! The `choz` command. Everything it does is in the library beside it — see
//! the crate documentation there for why the split exists.

fn main() {
    let code = match choz_ui::run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    // **Leave with `_exit`, not by returning.** By here the terminal is back
    // and every instance choz could shut down has been shut down (`App` is
    // dropped inside `run`). What is left are plugins choz *leaked* because
    // they never came back from their own teardown — Pianoteq 9 is one, with
    // a helper thread still parked on its condition variable. A normal exit
    // runs those libraries' C++ static destructors under that thread, and
    // Pianoteq answers with `std::system_error: Invalid argument` → abort, or
    // a join that never returns: choz crashing or freezing on the way out.
    // The kernel reclaims all of it anyway; only our own stdout needs a flush.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    unsafe { libc::_exit(code) }
}
