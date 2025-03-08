use dirs::home_dir;
use json::{JsonError, JsonValue};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{fs, io};
use sysinfo::{Process, System};

#[derive(Debug, Clone)]
pub struct JetBrainsToolboxInstallation {
    binary: PathBuf,
    channels: PathBuf,
    log: PathBuf,
}

#[derive(Debug)]
pub enum UpdateError {
    Io(io::Error),
    Json(JsonError),
    InvalidChannel,
    CouldNotTerminate(String),
}

impl JetBrainsToolboxInstallation {
    fn update_all_channels<F>(&self, mut operation: F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut JsonValue) -> Result<(), UpdateError>,
    {
        for file in fs::read_dir(&self.channels).map_err(UpdateError::Io)? {
            let file = file.map_err(UpdateError::Io)?;
            self.update_channel(file.path(), &mut operation)?;
        }
        Ok(())
    }

    fn update_channel<F>(&self, path: PathBuf, operation: &mut F) -> Result<(), UpdateError>
    where
        F: FnMut(&PathBuf, &mut JsonValue) -> Result<(), UpdateError>,
    {
        let mut file = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(UpdateError::Io)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).map_err(UpdateError::Io)?;
        let mut data = json::parse(&buf).map_err(UpdateError::Json)?;
        operation(&path, &mut data)?;
        // Seek to the start, dump, then truncate, to avoid re-opening the file
        file.seek(SeekFrom::Start(0)).map_err(UpdateError::Io)?;
        buf = data.dump();
        file.write_all(buf.as_bytes()).map_err(UpdateError::Io)?;
        let current_position = file.stream_position().map_err(UpdateError::Io)?;
        file.set_len(current_position).map_err(UpdateError::Io)?;

        Ok(())
    }

    fn start_minimized(&self) -> io::Result<Child> {
        Command::new(&self.binary).arg("--minimize").spawn()
    }
}

#[derive(Debug, Clone)]
pub enum FindError {
    NotFound,
    InvalidInstallation,
    NoHomeDir,
    UnsupportedOS(String),
}

#[cfg(target_os = "linux")]
pub fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    let home_dir = home_dir().ok_or(FindError::NoHomeDir)?;
    let dir = home_dir.join(".local/share/JetBrains/Toolbox");
    if !dir.exists() {
        return Err(FindError::NotFound);
    } else if !dir.is_dir() {
        // I don't know why there would ever be a normal file there but why not
        return Err(FindError::InvalidInstallation);
    }
    let binary = dir.join("bin/jetbrains-toolbox");
    if !binary.exists() {
        return Err(FindError::InvalidInstallation);
    }
    let channels = dir.join("channels");
    if !channels.is_dir() {
        return Err(FindError::InvalidInstallation);
    }
    let logs_dir = dir.join("logs");
    if !logs_dir.is_dir() {
        return Err(FindError::InvalidInstallation);
    }
    let log = logs_dir.join("toolbox.log"); // The log itself might not exist, so we don't check for it here

    Ok(JetBrainsToolboxInstallation {
        binary,
        channels,
        log,
    })
}

#[cfg(target_os = "windows")]
fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    FindError::UnsupportedOS("Windows") // TODO
}

#[cfg(target_os = "macos")]
fn find_jetbrains_toolbox() -> Result<JetBrainsToolboxInstallation, FindError> {
    FindError::UnsupportedOS("MacOS") // TODO
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn find_jetbrains_toolbox() -> Result<Installation, FindError> {
    // JetBrains Toolbox is not supported on mobile or BSD
    FindError::UnsupportedOS(std::env::OS)
}

fn kill_all() -> Result<bool, UpdateError> {
    let mut sys = System::new_all();
    sys.refresh_all();
    // TODO: this might not work on other platforms
    let processes = sys
        .processes()
        .values()
        .filter_map(|p| {
            let exe = p.exe()?; // Skip if no exe available
            let name = p.name();
            match exe.file_name().ok_or(UpdateError::CouldNotTerminate(
                "Error getting file_name".to_string(),
            )) {
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
        0 => false,
        _ => {
            println!("Found {} processes", processes.len());
            for process in processes {
                process.kill();
                process.wait();
            }
            true
        }
    })
}

pub fn update_jetbrains_toolbox(
    installation: JetBrainsToolboxInstallation,
) -> Result<(), UpdateError> {
    // Close the app if it's open
    let toolbox_was_open = kill_all()?;

    // Modify the configuration to enable automatic updates
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
                return Err(UpdateError::InvalidChannel);
            }
        }

        d["channel"]["autoUpdate"] = true.into();
        Ok(())
    })?;

    // Start the app in the background
    installation.start_minimized().map_err(UpdateError::Io)?;

    // Monitor the logs for possible updates, and wait until they're complete
    let mut updates: u32 = 0;
    let mut correct_checksums_expected: u32 = 0;
    let start_time = Instant::now();

    let file = File::open(&installation.log).map_err(UpdateError::Io)?;
    let mut file = BufReader::new(file);
    file.seek(SeekFrom::End(0)).map_err(UpdateError::Io)?;
    loop {
        // TODO: Unfortunately there is no log message indicating there are no updates; so waiting is necessary it looks like.
        //  Unless we can do something with "Downloaded fus-assistant.xml"? Maybe shorten the time to 1/2 seconds after that message, seems to be fine.
        if updates == 0 && start_time + Duration::from_secs(10) < Instant::now() {
            println!("No updates found.");
            break;
        }

        let curr_position = file.stream_position().map_err(UpdateError::Io)?;

        let mut line = String::new();
        file.read_line(&mut line).map_err(UpdateError::Io)?;

        if line.is_empty() {
            file.seek(SeekFrom::Start(curr_position))
                .map_err(UpdateError::Io)?;
            sleep(Duration::from_millis(100));
        } else {
            // If the download is already there, it won't say "Downloading from", but immediately "Correct checksum for".
            if line.contains("Correct checksum for") || line.contains("Downloading from") {
                if line.contains("Correct checksum for") && correct_checksums_expected > 0 {
                    correct_checksums_expected -= 1;
                    continue;
                }
                // Update started
                println!("Found an update, waiting until it finishes...");
                updates += 1;
                if line.contains("Downloading from") {
                    // We expect "Correct checksum for" to be broadcast exactly once after the downloading from.
                    correct_checksums_expected += 1;
                }
            } else if line.contains("Show notification") {
                // Update finished
                updates -= 1;
                if updates == 0 {
                    println!("Update finished, exiting...");
                    sleep(Duration::from_secs(2)); // Letting it finish up
                    break;
                } else {
                    println!("Update finished, waiting for other update(s) to finish")
                }
            }
        }
    }

    // Quit the app
    assert!(kill_all()?);

    // Reset the configuration
    installation.update_all_channels(|channel, d| {
        if !d.has_key("channel") {
            return Err(UpdateError::InvalidChannel);
        }
        if skipped_channels.contains(channel) {
            return Ok(());
        }
        d["channel"].remove("autoUpdate");
        Ok(())
    })?;

    // Restart the app if it was open
    if toolbox_was_open {
        installation.start_minimized().map_err(UpdateError::Io)?;
    }

    Ok(())
}
