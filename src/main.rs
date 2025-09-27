// TODO: CLI is blocked by https://github.com/rust-lang/cargo/issues/1982

// use clap::Parser;
use jetbrains_toolbox_updater::{find_jetbrains_toolbox, update_jetbrains_toolbox};

// #[derive(Parser, Debug)]
// #[command(name = "jetbrains-toolbox-updater", version)]
// #[command(about = "Updates JetBrains Toolbox IDE's on demand using some trickery")]
// #[command(propagate_version = true)]
// struct Cli {}

fn main() {
    // let _cli = Cli::parse();

    let installation = find_jetbrains_toolbox().unwrap(); // TODO: handle
    update_jetbrains_toolbox(installation).unwrap(); // TODO: handle
}
