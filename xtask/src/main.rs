use anyhow::{bail, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("wifi-hw") => xtask::wifi_hw::run_cli(args.collect()),
        Some(command) => bail!("unknown xtask command `{command}`"),
        None => {
            eprintln!("usage: cargo run -p xtask -- wifi-hw <list|doctor|run|matrix|companion>");
            Ok(())
        }
    }
}
