use dirs::home_dir;
use json::{JsonError, JsonValue};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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
    Json(JsonError),
    InvalidChannel,
    CouldNotTerminate(String),
    PrematureExit,
    DoubleToolboxSelfUpdate,
    StartupFusAssistantTimeout,
    DoubleStartupFusAssistant,
}

impl From<io::Error> for UpdateError {
    fn from(err: io::Error) -> UpdateError {
        UpdateError::Io(err)
    }
}

impl From<JsonError> for UpdateError {
    fn from(err: JsonError) -> UpdateError {
        UpdateError::Json(err)
    }
}

impl JetBrainsToolboxInstallation {
    fn update_all_channels<F>(&self, mut operation: F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut JsonValue) -> Result<(), UpdateError>,
    {
        for file in fs::read_dir(&self.channels)? {
            let file = file?;
            self.update_channel(file.path(), &mut operation)?;
        }
        Ok(())
    }

    fn update_channel<F>(&self, path: PathBuf, operation: &mut F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut JsonValue) -> Result<(), UpdateError>,
    {
        let mut file = File::options().read(true).write(true).open(&path)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let mut data = json::parse(&buf)?;
        operation(&path, &mut data)?;
        // Seek to the start, dump, then truncate, to avoid re-opening the file
        file.seek(SeekFrom::Start(0))?; // Seek
        buf = data.dump();
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
    // TODO: The comment below does not seem to make sense.
    // The log itself might not exist yet, so we don't check for it here
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
            )))
        }
        Some(first) => first,
    };

    // If they don't have the same `Exec` value, bail
    if !matches.all(|x| x == exec) {
        return Err(FindError::MultipleMismatchingDesktopFiles("Multiple desktop files called `jetbrains-toolbox.desktop` were found, and they have different values for Exec".to_string()));
    }

    let binary = exec.trim_end_matches(" %u");

    println!("Detected binary at {binary} from desktop file");

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

// Returns if it was open
fn kill_all() -> Result<bool, UpdateError> {
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
                "Error getting file_name".to_string(),
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
    println!("Killing Toolbox");
    let toolbox_was_open = kill_all()?;

    // Modify the configuration to enable automatic updates
    let skipped_channels = change_config(&installation)?;

    let redo = match actual_update(&installation) {
        Err(e) => {
            println!("Unexpected error encountered, resetting configuration to previous state");
            reset_config(&installation, skipped_channels)?;
            return Err(e);
        }
        Ok(redo) => redo,
    };

    // Reset the configuration
    reset_config(&installation, skipped_channels)?;

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

        if let Some(startup_time) = startup_time {
            // If 10 seconds pass from startup, we assume there are no updates
            if updates == 0 && startup_time + Duration::from_secs(10) < Instant::now() {
                println!("No updates found");
                break;
            }
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
            // TODO: Maybe just remove the downloading/checking checksum logic entirely,
            //  and assume we always download the update?
            // Each update consists of first downloading, then checking the checksum, then a lot of other things.
            //  If the download is already there, it won't say "Downloading from", it will skip that
            //  and immediately say "Correct checksum for".
            //  This means that a "Correct checksum for" after there was a "Downloading from"
            //  should not be considered as the start of a separate update.
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
            } else if line.contains("Show notification") {
                // Update finished
                updates -= 1;
                if updates == 0 {
                    println!("All updates finished, exiting in 2 seconds");
                    sleep(Duration::from_secs(2)); // Letting it finish up
                    break;
                } else {
                    println!("Update finished, waiting for other update(s) to finish")
                }
            } else if line.contains(
                "Shutting down. Reason: The updated app is starting, closing the current process",
            ) {
                // Toolbox (self-)update finished. This does say "Downloading from" when starting.
                // But since it restarted itself the state is messed up. We want to re-do the entire process once now.
                redo = true;
                println!(
                    "Toolbox (self-)update finished, update process will restart in 10 seconds"
                );
                // Letting it finish up. In this time, it will restart itself.
                //  We could theoretically wait for the restart, but that is less necessary, since
                //  self-updates are not very common compared to IDE updates.
                //  It is also (probably?) not necessary to wait for the restart to fully complete,
                //  we just want it to have finished any update-related shutdown or startup tasks.
                sleep(Duration::from_secs(10));
                break;
            } else if line.contains("Downloaded fus-assistant.xml") {
                if startup_time.is_some() {
                    // We expect to get it once
                    return Err(UpdateError::DoubleStartupFusAssistant);
                }
                println!("Toolbox started");
                startup_time = Some(Instant::now())
            }
        }
    }

    // Quit the app
    println!("Killing Toolbox");
    if !kill_all()? {
        // We expect it to be running.
        return Err(UpdateError::PrematureExit);
    }

    Ok(redo)
}

fn change_config(installation: &JetBrainsToolboxInstallation) -> Result<Vec<PathBuf>, UpdateError> {
    let mut skipped_channels = vec![];
    installation.update_all_channels(|channel, d| {
        if !d.has_key("channel") {
            return Err(UpdateError::InvalidChannel);
        }
        if d["channel"].has_key("autoUpdate") {
            if d["channel"]["autoUpdate"] == true {
                // This channel is already auto-updating, we won't touch the configuration in this case
                skipped_channels.push(channel.clone());
                return Ok(());
            } else {
                // We expect autoUpdate to be missing if it's false
                return Err(UpdateError::InvalidChannel);
            }
        }

        d["channel"]["autoUpdate"] = true.into();
        Ok(())
    })?;
    Ok(skipped_channels)
}

fn reset_config(
    installation: &JetBrainsToolboxInstallation,
    skipped_channels: Vec<PathBuf>,
) -> Result<(), UpdateError> {
    installation.update_all_channels(|channel, d| {
        if !d.has_key("channel") {
            return Err(UpdateError::InvalidChannel);
        }
        if skipped_channels.contains(channel) {
            // Skip if it was skipped at the start as well
            return Ok(());
        }
        d["channel"].remove("autoUpdate");
        Ok(())
    })?;
    Ok(())
}
