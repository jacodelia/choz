//! The `choz` command. Everything it does is in the library beside it — see
//! the crate documentation there for why the split exists.

fn main() -> anyhow::Result<()> {
    choz_ui::run()
}
