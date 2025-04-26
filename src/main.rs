// TODO: CLI is blocked by https://github.com/rust-lang/cargo/issues/1982

// use clap::Parser;
use jetbrains_toolbox_updater::{find_jetbrains_toolbox, update_jetbrains_toolbox};

// pub mod built_info {
//     include!(concat!(env!("OUT_DIR"), "/built.rs"));
// }
//
// const VERSION: &str = built_info::PKG_VERSION;
//
// #[derive(Parser, Debug)]
// #[command(name = "jetbrains-toolbox-updater", author, long_version = crate::VERSION)]
// #[command(about = "Updates JetBrains Toolbox IDE's on demand using some trickery")]
// #[command(propagate_version = true)]
// struct Cli {}

fn main() {
    // let _cli = Cli::parse();

    let installation = find_jetbrains_toolbox().unwrap(); // TODO: handle
    update_jetbrains_toolbox(installation).unwrap(); // TODO: handle
}
