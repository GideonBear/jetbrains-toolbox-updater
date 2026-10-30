// TODO: CLI is blocked by https://github.com/rust-lang/cargo/issues/1982

// use clap::Parser;
use jetbrains_toolbox_updater::{UpdateError, find_jetbrains_toolbox, update_jetbrains_toolbox};

// #[derive(Parser, Debug)]
// #[command(name = "jetbrains-toolbox-updater", version)]
// #[command(about = "Updates JetBrains Toolbox IDE's on demand using some trickery")]
// #[command(propagate_version = true)]
// struct Cli {}

fn main() {
    // let _cli = Cli::parse();

    let installation = find_jetbrains_toolbox().unwrap(); // TODO: handle
    match update_jetbrains_toolbox(installation) {
        // The usual exit code after Ctrl+C (128 + SIGINT)
        Err(UpdateError::Interrupted) => std::process::exit(130),
        result => result.unwrap(), // TODO: handle
    }
}
