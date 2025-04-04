
# jetbrains-toolbox-updater

Application and library that updates [JetBrains Toolbox](https://www.jetbrains.com/toolbox-app/) IDE's on demand using some trickery.
It currently supports Linux only, but Windows support is planned.
## Installation

jetbrains-toolbox-updater is included as part of [topgrade](https://github.com/topgrade-rs/topgrade), but it can also be installed seperately using cargo:

```bash
cargo install jetbrains-toolbox-updater
```
And used by running:
```bash
jetbrains-toolbox-updater
```

## How it works
The process is as follows:

1. Close JetBrains Toolbox if it's open
2. Modify the configuration to enable automatic updates
3. Start JetBrains Toolbox in the background
4. Monitor the logs for possible updates, and wait until they're complete
5. Quit and reset the configuration
6. Restart JetBrains Toolbox if it was open
