#[cfg(target_os = "linux")]
use log::debug;
#[cfg(target_os = "linux")]
use std::env::home_dir;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{fs, io};
use sysinfo::{Process, System};

#[derive(Debug, Clone)]
pub struct JetBrainsToolboxInstallation {
    binary: PathBuf,
    channels: PathBuf, // The folder containing configuration for individual IDE's
    log: PathBuf,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum UpdateError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidChannel,
    CouldNotTerminate(String),
    PrematureExit,
    DoubleToolboxSelfUpdate,
    StartupFusAssistantTimeout,
    DoubleStartupFusAssistant,
    BadLog(String),
}

impl From<io::Error> for UpdateError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for UpdateError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl JetBrainsToolboxInstallation {
    fn update_all_channels<F>(&self, mut operation: F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut serde_json::Value) -> Result<(), UpdateError>,
    {
        for file in fs::read_dir(&self.channels)? {
            let file = file?;
            self.update_channel(file.path(), &mut operation)?;
        }
        Ok(())
    }

    fn update_channel<F>(&self, path: PathBuf, operation: &mut F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut serde_json::Value) -> Result<(), UpdateError>,
    {
        let mut file = File::options().read(true).write(true).open(&path)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let mut data: serde_json::Value = serde_json::from_str(&buf)?;
        operation(&path, &mut data)?;
        // Seek to the start, dump, then truncate, to avoid re-opening the file
        file.seek(SeekFrom::Start(0))?; // Seek
        buf = serde_json::to_string_pretty(&data)?;
        file.write_all(buf.as_bytes())?; // Dump
        let current_position = file.stream_position()?;
        file.set_len(current_position)?; // Truncate

        Ok(())
    }

    fn start_minimized(&self) -> io::Result<Child> {
        Command::new(&self.binary).arg("--minimize").spawn()
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FindError {
    NotFound,
    InvalidInstallation,
    NoHomeDir,
    UnsupportedOS(String),
    NoDesktopFile(String),
    DesktopFileMissingExec,
    MultipleMismatchingDesktopFiles(String),
}

#[cfg(target_os = "linux")]
pub fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    let home_dir = home_dir().ok_or(FindError::NoHomeDir)?;
    // TODO: allow custom dir
    let local_share = home_dir.join(".local/share");
    let dir = local_share.join("JetBrains/Toolbox");
    if !dir.exists() {
        return Err(FindError::NotFound);
    } else if !dir.is_dir() {
        // I don't know why there would ever be a normal file there but why not
        return Err(FindError::InvalidInstallation);
    }
    // In previous versions, the binary would copy itself to {dir}/bin
    let mut binary = dir.join("bin/jetbrains-toolbox");
    if !binary.exists() {
        // In newer versions, it doesn't. We use the desktop file to find
        // the location of the binary, since the user can put it anywhere.
        binary = get_binary_from_desktop(&binary)?;
    }
    let channels = dir.join("channels");
    if !channels.is_dir() {
        return Err(FindError::InvalidInstallation);
    }
    let logs_dir = dir.join("logs");
    if !logs_dir.is_dir() {
        return Err(FindError::InvalidInstallation);
    }
    // The log itself might not exist yet, so we don't check if it exists
    let log = logs_dir.join("toolbox.latest.log");

    Ok(JetBrainsToolboxInstallation {
        binary,
        channels,
        log,
    })
}

#[cfg(target_os = "linux")]
fn get_binary_from_desktop(orig_binary: &Path) -> Result<PathBuf, FindError> {
    let entries = freedesktop_desktop_entry::Iter::new(freedesktop_desktop_entry::default_paths())
        .entries::<String>(None)
        .collect::<Vec<_>>();

    let mut matches = entries
        .iter()
        .filter(|entry| {
            entry
                .path
                .file_name()
                .expect("Invalid desktop entry file; terminates in `..`")
                == "jetbrains-toolbox.desktop"
        })
        .map(|entry| entry.exec().ok_or(FindError::DesktopFileMissingExec))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();

    // If multiple desktop files are found but they have the same `Exec` value, it's fine
    let exec = match matches.next() {
        None => {
            return Err(FindError::NoDesktopFile(format!(
                "No binary was found at {}, and no desktop file named `jetbrains-toolbox.desktop` was found",
                orig_binary.display(),
            )));
        }
        Some(first) => first,
    };

    // If they don't have the same `Exec` value, bail
    if !matches.all(|x| x == exec) {
        return Err(FindError::MultipleMismatchingDesktopFiles("Multiple desktop files called `jetbrains-toolbox.desktop` were found, and they have different values for Exec".to_owned()));
    }

    let binary = exec.trim_end_matches(" %u");

    debug!("Detected binary at {binary} from desktop file");

    Ok(PathBuf::from(binary))
}

#[cfg(target_os = "windows")]
pub fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    Err(FindError::UnsupportedOS("Windows".to_string())) // TODO
}

#[cfg(target_os = "macos")]
pub fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    Err(FindError::UnsupportedOS("MacOS".to_string())) // TODO
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    // JetBrains Toolbox is not supported on mobile or BSD
    Err(FindError::UnsupportedOS(std::env::consts::OS.to_string()))
}

/// Returns if it was open
fn kill_all() -> Result<bool, UpdateError> {
    println!("Killing Toolbox");
    let mut sys = System::new_all();
    sys.refresh_all();
    // TODO: this might not work on other platforms; look at this when adding support for Windows/MacOS
    let processes = sys
        .processes()
        .values()
        .filter_map(|p| {
            let exe = p.exe()?; // Skip if no exe available
            let name = p.name();
            match exe.file_name().ok_or(UpdateError::CouldNotTerminate(
                "Error getting file_name".to_owned(),
            )) {
                // There are some weird quirks with processes here.
                //  psutil in python never had a problem with this, but sysinfo
                //  results in three different processes.
                //  In addition to that, there are some other child processes with weird names,
                //  and the names are cut off to 15 characters.
                //  Doing it like this results in killing only those three, which is
                //  probably the best approach.
                Ok(file_name)
                    if file_name == "jetbrains-toolbox"
                        && name.to_str()?.starts_with("jetbrains") =>
                {
                    Some(Ok(p))
                }
                Ok(_) => None,          // Skip items that don't match
                Err(e) => Some(Err(e)), // Propagate the error
            }
        })
        .collect::<Result<Vec<&Process>, UpdateError>>()?;
    Ok(match processes.len() {
        0 => false, // Was not open
        _ => {
            for process in processes {
                process.kill();
                process.wait();
            }
            true // Was open
        }
    })
}

pub fn update_jetbrains_toolbox(
    installation: JetBrainsToolboxInstallation,
) -> Result<(), UpdateError> {
    _update_jetbrains_toolbox::<false>(installation)
}

fn _update_jetbrains_toolbox<const IS_RECURSIVE: bool>(
    installation: JetBrainsToolboxInstallation,
) -> Result<(), UpdateError> {
    // Close the app if it's open
    let toolbox_was_open = kill_all()?;

    // Modify the configuration to enable automatic updates
    let mut modified_channels = vec![];
    let redo = match change_config(&installation, &mut modified_channels)
        .and_then(|()| actual_update(&installation))
    {
        Err(e) => {
            println!("Unexpected error encountered, restoring Toolbox to its previous state");
            if let Err(restore_err) = restore(&installation, toolbox_was_open, &modified_channels) {
                println!("Error while restoring Toolbox's previous state: {restore_err:?}");
            }
            return Err(e);
        }
        Ok(redo) => redo,
    };

    // Reset the configuration
    reset_config(&installation, &modified_channels)?;

    // Restart the app if it was open
    if toolbox_was_open {
        println!("Re-opening Toolbox");
        installation.start_minimized()?;
    }

    if redo {
        // We want to redo. We reset the configuration and re-opened Toolbox
        //  (technically not needed, but this makes the process a bit simpler)
        //  So now we just want to redo the entire process.
        //  We do this by calling this function recursively.

        if IS_RECURSIVE {
            // Except if this was already recursive, then there must be something very wrong.
            return Err(UpdateError::DoubleToolboxSelfUpdate);
        }

        _update_jetbrains_toolbox::<true>(installation)
    } else {
        Ok(())
    }
}

/// Returns redo
fn actual_update(installation: &JetBrainsToolboxInstallation) -> Result<bool, UpdateError> {
    let mut redo = false;

    // Start the app in the background
    println!("Starting Toolbox");
    installation.start_minimized()?;

    // Monitor the logs for possible updates, and wait until they're complete
    let mut updates: u32 = 0;
    let mut correct_checksums_expected: u32 = 0;
    let start_time = Instant::now();
    let mut startup_time = None;

    let file = File::open(&installation.log)?;
    let mut file = BufReader::new(file);
    file.seek(SeekFrom::End(0))?;
    loop {
        // If 60 seconds pass without the startup event happening, something's wrong.
        //  Even if there are updates found.
        if startup_time.is_none() && start_time + Duration::from_secs(60) < Instant::now() {
            return Err(UpdateError::StartupFusAssistantTimeout);
        }

        if let Some(startup_time) = startup_time
            // If 10 seconds pass from startup, we assume there are no updates
            && updates == 0 && startup_time + Duration::from_secs(10) < Instant::now()
        {
            println!("No updates found");
            break;
        }

        let curr_position = file.stream_position()?;

        // Read a line
        let mut line = String::new();
        file.read_line(&mut line)?;

        if line.is_empty() {
            // There is no new full line, so seek back to before the (possibly partial) line was read,
            //  and sleep for a bit.
            file.seek(SeekFrom::Start(curr_position))?;
            sleep(Duration::from_millis(100));
        } else {
            // Each update consists of first downloading, then checking the checksum, then a lot of other things.
            //  If the download is already there, it won't say "Downloading from", it will skip that
            //  and immediately say "Correct checksum for".
            //  This means that a "Correct checksum for" after there was a "Downloading from"
            //  should not be considered as the start of a separate update.
            //  We need to support this, because pre-downloaded updates are a real thing that can
            //  happen, e.g. with self-updates.
            if line.contains("Correct checksum for") || line.contains("Downloading from") {
                if line.contains("Correct checksum for") && correct_checksums_expected > 0 {
                    println!("Verified a checksum for an update that was started earlier");
                    correct_checksums_expected -= 1;
                    continue;
                }
                // Update started
                println!("Found an update, waiting until it finishes");
                updates += 1;
                if line.contains("Downloading from") {
                    // We expect "Correct checksum for" to be broadcast exactly once after the "Downloading from".
                    correct_checksums_expected += 1;
                }
            } else if line.contains("update-notification") {
                // Update finished
                updates -= 1;
                if updates == 0 {
                    println!("All updates finished, exiting in 30 seconds");
                    sleep(Duration::from_secs(30)); // Letting it finish up the "Configuring" step
                    break;
                } else {
                    println!("Update finished, waiting for other update(s) to finish");
                }
            } else if line.contains("Awaiting user action or background state to install.") {
                println!(
                    "Toolbox self-update is ready. The self-update will apply automatically \
                    in 60 seconds if you don't open Toolbox, but you can also click \
                    'Restart Toolbox App to complete update' in the settings menu now."
                );
            } else if line.contains(
                "Shutting down. Reason: The updated app is starting, closing the current process",
            ) {
                // The self-update does abide by "Downloading from" and "Correct checksum for", but
                // since it restarted itself, the state is messed up. We want to redo the entire process once now.
                redo = true;
                // Somehow even manually killing the process it's waiting for doesn't convince it.
                // The only way to continue the self-update process is by waiting for the timeout.
                println!(
                    "Toolbox self-update download finished. We will now wait 20 seconds for \
                    waitForPid to timeout, then we will wait another 10 seconds to make sure the \
                    self-update is fully installed. Then we will restart the update process once."
                );
                sleep(Duration::from_secs(
                    20
                    // Letting it finish up. In this time, it will restart itself.
                    //  We could theoretically wait for the restart, but that is less necessary, since
                    //  self-updates are not very common compared to IDE updates.
                    //  It is also (probably?) not necessary to wait for the restart to fully complete,
                    //  we just want it to have finished any update-related shutdown or startup tasks.
                    + 10,
                ));
                break;
            } else if line.contains("Downloaded fus-assistant.xml") {
                if startup_time.is_some() {
                    // We expect to get it once
                    return Err(UpdateError::DoubleStartupFusAssistant);
                }
                println!("Toolbox started");
                startup_time = Some(Instant::now());
            }
        }
    }

    // Quit the app
    if !kill_all()? {
        // We expect it to be running.
        return Err(UpdateError::PrematureExit);
    }

    Ok(redo)
}

/// Puts Toolbox back the way it was before the update started
fn restore(
    installation: &JetBrainsToolboxInstallation,
    toolbox_was_open: bool,
    modified_channels: &[PathBuf],
) -> Result<(), UpdateError> {
    // Close our instance, if it's running
    kill_all()?;
    reset_config(installation, modified_channels)?;
    if toolbox_was_open {
        println!("Re-opening Toolbox");
        installation.start_minimized()?;
    }
    Ok(())
}

/// Adds every channel it enables automatic updates for to `modified_channels`,
/// even if it fails halfway through
fn change_config(
    installation: &JetBrainsToolboxInstallation,
    modified_channels: &mut Vec<PathBuf>,
) -> Result<(), UpdateError> {
    installation.update_all_channels(|channel, d| {
        if !d["channel"].is_object() {
            return Err(UpdateError::InvalidChannel);
        }
        if let Some(auto_update) = d["channel"].get("autoUpdate") {
            if auto_update.as_bool() == Some(true) {
                // This channel is already auto-updating, we won't touch the configuration in this case
                return Ok(());
            } else {
                // We expect autoUpdate to be missing if it's false
                return Err(UpdateError::InvalidChannel);
            }
        }

        d["channel"]["autoUpdate"] = true.into();
        modified_channels.push(channel.clone());
        Ok(())
    })
}

fn reset_config(
    installation: &JetBrainsToolboxInstallation,
    modified_channels: &[PathBuf],
) -> Result<(), UpdateError> {
    installation.update_all_channels(|channel, d| {
        if d.get("channel").is_none() {
            return Err(UpdateError::InvalidChannel);
        }
        if !modified_channels.contains(channel) {
            // Skip if we didn't enable automatic updates for it
            return Ok(());
        }
        if let Some(channel_obj) = d.get_mut("channel").and_then(|v| v.as_object_mut()) {
            channel_obj.remove("autoUpdate");
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An installation whose channels folder is a fresh temporary directory
    fn installation_with_channels(
        name: &str,
        channels: &[(&str, &str)],
    ) -> JetBrainsToolboxInstallation {
        let dir = std::env::temp_dir().join(format!(
            "jetbrains-toolbox-updater-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (file_name, contents) in channels {
            fs::write(dir.join(file_name), contents).unwrap();
        }
        JetBrainsToolboxInstallation {
            binary: PathBuf::from("jetbrains-toolbox"),
            channels: dir,
            log: PathBuf::from("toolbox.latest.log"),
        }
    }

    fn auto_update(installation: &JetBrainsToolboxInstallation, file_name: &str) -> Option<bool> {
        let buf = fs::read_to_string(installation.channels.join(file_name)).unwrap();
        let data: serde_json::Value = serde_json::from_str(&buf).unwrap();
        data["channel"]
            .get("autoUpdate")
            .map(|v| v.as_bool().unwrap())
    }

    #[test]
    fn change_and_reset_config() {
        let installation = installation_with_channels(
            "change-and-reset",
            &[
                ("manual.json", r#"{"channel": {"id": "a"}}"#),
                (
                    "auto.json",
                    r#"{"channel": {"id": "b", "autoUpdate": true}}"#,
                ),
            ],
        );

        let mut modified_channels = vec![];
        change_config(&installation, &mut modified_channels).unwrap();
        assert_eq!(
            modified_channels,
            [installation.channels.join("manual.json")]
        );
        assert_eq!(auto_update(&installation, "manual.json"), Some(true));
        assert_eq!(auto_update(&installation, "auto.json"), Some(true));

        reset_config(&installation, &modified_channels).unwrap();
        assert_eq!(auto_update(&installation, "manual.json"), None);
        assert_eq!(auto_update(&installation, "auto.json"), Some(true));

        fs::remove_dir_all(&installation.channels).unwrap();
    }

    #[test]
    fn reset_config_after_partial_change_config() {
        // Several valid channels, so some are most likely visited before the invalid one.
        //  The assertions hold whatever order the directory is read in.
        let mut channels = vec![
            (
                "auto.json",
                r#"{"channel": {"id": "auto", "autoUpdate": true}}"#,
            ),
            (
                "invalid.json",
                r#"{"channel": {"id": "invalid", "autoUpdate": false}}"#,
            ),
        ];
        let manual: Vec<_> = (0..8).map(|i| format!("manual-{i}.json")).collect();
        for file_name in &manual {
            channels.push((file_name, r#"{"channel": {"id": "manual"}}"#));
        }
        let installation = installation_with_channels("partial", &channels);

        let mut modified_channels = vec![];
        let result = change_config(&installation, &mut modified_channels);
        assert!(matches!(result, Err(UpdateError::InvalidChannel)));
        // Exactly the channels that now have autoUpdate set are recorded
        for file_name in &manual {
            assert_eq!(
                modified_channels.contains(&installation.channels.join(file_name)),
                auto_update(&installation, file_name) == Some(true),
            );
        }

        reset_config(&installation, &modified_channels).unwrap();
        for file_name in &manual {
            assert_eq!(auto_update(&installation, file_name), None);
        }
        assert_eq!(auto_update(&installation, "auto.json"), Some(true));
        assert_eq!(auto_update(&installation, "invalid.json"), Some(false));

        fs::remove_dir_all(&installation.channels).unwrap();
    }
}
