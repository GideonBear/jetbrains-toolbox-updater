use jetbrains_toolbox_updater::{find_jetbrains_toolbox, update_jetbrains_toolbox};

fn main() {
    let installation = find_jetbrains_toolbox().unwrap(); // TODO: handle
    update_jetbrains_toolbox::<false>(installation).unwrap(); // TODO: handle
}
