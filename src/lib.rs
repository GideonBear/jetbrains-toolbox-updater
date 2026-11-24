#[cfg(target_os = "linux")]
use log::debug;
use std::env::home_dir;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    Interrupted,
    UpdateTimeout,
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
        let mut command = Command::new(&self.binary);
        // Toolbox quits on SIGINT, so keep it out of the terminal's process group,
        //  where it would receive the user's Ctrl+C
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        // Don't inherit our working directory: it may be deleted while Toolbox is still
        //  starting (Topgrade runs steps in a temporary directory), which makes it crash
        if let Some(home_dir) = home_dir() {
            command.current_dir(home_dir);
        }
        command.arg("--minimize").spawn()
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

/// Sets `interrupted` on Ctrl+C while it exists. A previously installed handler (e.g. Topgrade's)
/// is still called.
struct CtrlCGuard {
    interrupted: Arc<AtomicBool>,
    #[cfg(unix)]
    id: signal_hook::SigId,
}

impl CtrlCGuard {
    fn new() -> Result<Self, UpdateError> {
        let interrupted = Arc::new(AtomicBool::new(false));
        Ok(Self {
            #[cfg(unix)]
            id: signal_hook::flag::register(signal_hook::consts::SIGINT, interrupted.clone())?,
            interrupted,
        })
    }
}

impl Drop for CtrlCGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        signal_hook::low_level::unregister(self.id);
    }
}

/// Sleeps for `duration`, or returns early with [`UpdateError::Interrupted`] on Ctrl+C
fn sleep_unless_interrupted(
    duration: Duration,
    interrupted: &AtomicBool,
) -> Result<(), UpdateError> {
    let start = Instant::now();
    while start.elapsed() < duration {
        if interrupted.load(Ordering::Relaxed) {
            return Err(UpdateError::Interrupted);
        }
        sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub fn update_jetbrains_toolbox(
    installation: JetBrainsToolboxInstallation,
) -> Result<(), UpdateError> {
    let ctrl_c = CtrlCGuard::new()?;
    _update_jetbrains_toolbox::<false>(installation, &ctrl_c.interrupted)
}

fn _update_jetbrains_toolbox<const IS_RECURSIVE: bool>(
    installation: JetBrainsToolboxInstallation,
    interrupted: &AtomicBool,
) -> Result<(), UpdateError> {
    // Close the app if it's open
    let toolbox_was_open = kill_all()?;

    // Modify the configuration to enable automatic updates
    let mut modified_channels = vec![];
    let redo = match change_config(&installation, &mut modified_channels)
        .and_then(|()| actual_update(&installation, interrupted))
    {
        Err(e) => {
            if matches!(e, UpdateError::Interrupted) {
                println!("Interrupted, restoring Toolbox to its previous state");
            } else {
                println!("Unexpected error encountered, restoring Toolbox to its previous state");
            }
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

        _update_jetbrains_toolbox::<true>(installation, interrupted)
    } else {
        Ok(())
    }
}

/// How long to wait after the last update finished, since Toolbox sometimes starts another one
///  (e.g. it can update the same IDE twice in a row)
const SETTLE_TIME: Duration = Duration::from_secs(15);
/// How long to wait when the log doesn't add up. E.g. a major version update (with a new
///  configuration folder) doesn't run the post-install step, so it never reports finishing it.
const QUIET_TIME: Duration = Duration::from_secs(2 * 60);
/// How long a download can go without any update-related log line before we give up
const STALL_TIME: Duration = Duration::from_secs(10 * 60);

/// Something update-related that Toolbox logged
#[derive(Debug, PartialEq)]
enum UpdateEvent {
    DownloadStarted(String),
    DownloadVerified(String),
    /// A checksum was verified for a file we didn't see being downloaded, e.g. because it was
    ///  downloaded earlier
    PreDownloaded(String),
    InstallStarted(String),
    InstallFinished,
    /// An IDE can't be updated because it's running
    InstallBlocked(String),
    SelfUpdatePending,
    /// Any other line that shows an update is still progressing
    Progress,
}

/// Keeps track of the updates Toolbox is working on, based on its log.
///
/// Toolbox doesn't log a reliable "update finished" line, so instead we keep track of what's
///  still in progress: files being downloaded, downloaded files that still need to be
///  installed, and installs that are still running their post-install step.
#[derive(Debug, Default)]
struct UpdateProgress {
    /// Files that are being downloaded, but whose checksum hasn't been verified yet
    downloads: Vec<String>,
    /// Files whose checksum has been verified. Each one is installed, except patches: a chain of
    ///  patches (e.g. `IU-1-2-patch-unix.jar` and `IU-2-3-patch-unix.jar`) is installed once.
    verified: Vec<String>,
    installs_started: u32,
    installs_finished: u32,
    /// IDE's that can't be updated because they're running
    blocked: Vec<String>,
    /// Whether a Toolbox self-update is waiting to be applied
    self_update_pending: bool,
    /// When the last update-related line was logged, if there was one
    last_activity: Option<Instant>,
}

impl UpdateProgress {
    fn on_line(&mut self, line: &str, now: Instant) -> Option<UpdateEvent> {
        let event = match parse_update_event(line)? {
            UpdateEvent::DownloadVerified(file) if !self.downloads.contains(&file) => {
                UpdateEvent::PreDownloaded(file)
            }
            event => event,
        };
        match &event {
            UpdateEvent::DownloadStarted(file) => {
                // A download that is resumed is logged again
                if !self.downloads.contains(file) {
                    self.downloads.push(file.clone());
                }
            }
            UpdateEvent::DownloadVerified(file) | UpdateEvent::PreDownloaded(file) => {
                self.downloads.retain(|f| f != file);
                // The self-update isn't installed like the IDE's are
                if !file.starts_with("jetbrains-toolbox-") {
                    self.verified.push(file.clone());
                }
            }
            UpdateEvent::InstallStarted(_) => self.installs_started += 1,
            UpdateEvent::InstallFinished => self.installs_finished += 1,
            UpdateEvent::InstallBlocked(tool) => {
                if !self.blocked.contains(tool) {
                    self.blocked.push(tool.clone());
                }
            }
            UpdateEvent::SelfUpdatePending => self.self_update_pending = true,
            UpdateEvent::Progress => {}
        }
        self.last_activity = Some(now);
        Some(event)
    }

    /// The number of installs we expect, based on the verified files
    fn expected_installs(&self) -> usize {
        let patches: Vec<(&str, &str, &str)> = self
            .verified
            .iter()
            .filter_map(|f| parse_patch(f))
            .collect();
        let full_downloads = self.verified.len() - patches.len();
        // Count the last patch of every chain: the one that no other patch continues from
        let chains = patches
            .iter()
            .filter(|(product, _, to)| {
                !patches
                    .iter()
                    .any(|(other_product, from, _)| other_product == product && from == to)
            })
            .count();
        full_downloads + chains
    }

    /// Whether there were updates and they're all finished
    fn finished(&self, now: Instant) -> bool {
        let Some(last_activity) = self.last_activity else {
            return false;
        };
        if !self.downloads.is_empty() || self.self_update_pending {
            return false;
        }
        // Toolbox installs the update of a running IDE once it's closed, so wait for that
        let installs_started = self.installs_started as usize;
        if !self.blocked.is_empty() && installs_started < self.expected_installs() {
            return false;
        }
        let quiet = now.saturating_duration_since(last_activity);
        let all_installed = installs_started >= self.expected_installs()
            && self.installs_finished >= self.installs_started;
        if all_installed {
            quiet >= SETTLE_TIME
        } else {
            quiet >= QUIET_TIME
        }
    }

    /// Whether a download has been going on for too long without any progress
    fn stalled(&self, now: Instant) -> bool {
        self.last_activity.is_some_and(|last_activity| {
            !self.downloads.is_empty() && now.saturating_duration_since(last_activity) >= STALL_TIME
        })
    }
}

/// Splits a patch file name like `IU-261.1-261.2-patch-unix.jar` into `("IU", "261.1", "261.2")`
fn parse_patch(file: &str) -> Option<(&str, &str, &str)> {
    let (builds, _) = file.split_once("-patch-")?;
    let (rest, to) = builds.rsplit_once('-')?;
    let (product, from) = rest.rsplit_once('-')?;
    Some((product, from, to))
}

fn parse_update_event(line: &str) -> Option<UpdateEvent> {
    // The file name of a path or URL
    fn file_name(s: &str) -> &str {
        let s = s.split('?').next().unwrap_or(s);
        s.rsplit('/').next().unwrap_or(s)
    }
    // The first word after `marker`
    fn word_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
        let (_, rest) = line.split_once(marker)?;
        rest.split_whitespace().next()
    }

    if let Some(url) = word_after(line, "Downloading from ") {
        let file = file_name(url);
        // Toolbox also downloads icons, which don't get a checksum
        let image = [".svg", ".png", ".jpg", ".jpeg", ".ico"]
            .iter()
            .any(|extension| file.ends_with(extension));
        (!image).then(|| UpdateEvent::DownloadStarted(file.to_owned()))
    } else if let Some(path) = word_after(line, "Correct checksum for ") {
        // Downloads are saved as `{hash}-{file name}`
        let saved_as = file_name(path);
        let file = saved_as.split_once('-').map_or(saved_as, |(_, file)| file);
        Some(UpdateEvent::DownloadVerified(file.to_owned()))
    } else if line.contains(" atomically") && line.contains("Moved ") {
        // `Moved {cache}/temp/{build}-{number} -> {install location} atomically`.
        //  Other things are moved too (e.g. vmoptions files), but not from the temp folder.
        let from = word_after(line, "Moved ")?;
        let to = word_after(line, " -> ")?;
        let from_temp = Path::new(from)
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == "temp");
        from_temp.then(|| UpdateEvent::InstallStarted(file_name(to).to_owned()))
    } else if line.contains("idea.log end contents") {
        // Logged once per install, after its post-install step (updating plugins)
        Some(UpdateEvent::InstallFinished)
    } else if line.contains("Couldn't uninstall InstalledTool(") {
        // `Couldn't uninstall InstalledTool(toolId=RustRover, toolName=RustRover, ...): Running`
        let tool = word_after(line, "toolName=")?.trim_end_matches(',');
        Some(UpdateEvent::InstallBlocked(tool.to_owned()))
    } else if line.contains("Awaiting user action or background state to install.") {
        Some(UpdateEvent::SelfUpdatePending)
    } else if line.contains("with ToolPatch(") || line.contains(" args: update") {
        Some(UpdateEvent::Progress)
    } else {
        None
    }
}

/// Returns redo
fn actual_update(
    installation: &JetBrainsToolboxInstallation,
    interrupted: &AtomicBool,
) -> Result<bool, UpdateError> {
    let mut redo = false;

    // Start the app in the background
    println!("Starting Toolbox");
    installation.start_minimized()?;

    // Monitor the logs for possible updates, and wait until they're complete
    let mut progress = UpdateProgress::default();
    let start_time = Instant::now();
    let mut startup_time = None;

    let file = File::open(&installation.log)?;
    let mut file = BufReader::new(file);
    file.seek(SeekFrom::End(0))?;
    loop {
        if interrupted.load(Ordering::Relaxed) {
            return Err(UpdateError::Interrupted);
        }

        // If 60 seconds pass without the startup event happening, something's wrong.
        //  Even if there are updates found.
        if startup_time.is_none() && start_time + Duration::from_secs(60) < Instant::now() {
            return Err(UpdateError::StartupFusAssistantTimeout);
        }

        if let Some(startup_time) = startup_time
            // If 10 seconds pass from startup, we assume there are no updates
            && progress.last_activity.is_none() && startup_time + Duration::from_secs(10) < Instant::now()
        {
            println!("No updates found");
            break;
        }

        if progress.finished(Instant::now()) {
            println!("All updates finished");
            break;
        }
        if progress.stalled(Instant::now()) {
            return Err(UpdateError::UpdateTimeout);
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
            // Most update-related lines are handled by `progress`
            if let Some(event) = progress.on_line(&line, Instant::now()) {
                match event {
                    UpdateEvent::DownloadStarted(file) => {
                        println!("Found an update, downloading {file}")
                    }
                    UpdateEvent::DownloadVerified(file) => println!("Downloaded {file}"),
                    UpdateEvent::PreDownloaded(file) => {
                        println!("Found an update that was already downloaded: {file}")
                    }
                    UpdateEvent::InstallStarted(tool) => println!("Installing {tool}"),
                    UpdateEvent::InstallBlocked(tool) => println!(
                        "Warning: {tool} can't be updated while it's running. Waiting for you to close it"
                    ),
                    UpdateEvent::InstallFinished => {
                        println!(
                            "Finished installing an update, waiting for possible other updates"
                        )
                    }
                    UpdateEvent::SelfUpdatePending => println!(
                        "Toolbox self-update is ready. The self-update will apply automatically \
                        in 60 seconds if you don't open Toolbox, but you can also click \
                        'Restart Toolbox App to complete update' in the settings menu now."
                    ),
                    UpdateEvent::Progress => {}
                }
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
                sleep_unless_interrupted(
                    Duration::from_secs(
                        20
                    // Letting it finish up. In this time, it will restart itself.
                    //  We could theoretically wait for the restart, but that is less necessary, since
                    //  self-updates are not very common compared to IDE updates.
                    //  It is also (probably?) not necessary to wait for the restart to fully complete,
                    //  we just want it to have finished any update-related shutdown or startup tasks.
                    + 10,
                    ),
                    interrupted,
                )?;
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

    /// Feeds a Toolbox log to a new `UpdateProgress`, using the log's own timestamps. Panics if it
    ///  would've considered the updates finished before an update-related line.
    fn replay(log: &str) -> UpdateProgress {
        let start = Instant::now();
        let mut progress = UpdateProgress::default();
        for line in log.lines() {
            // `{version} {pid} {date} {hours}:{minutes}:{seconds} ...`, all on one day
            let time = line.split_whitespace().nth(3).unwrap();
            let seconds = time
                .split(':')
                .map(|part| part.parse::<f64>().unwrap())
                .fold(0.0, |total, part| total * 60.0 + part);
            let now = start + Duration::from_secs_f64(seconds);
            if parse_update_event(line).is_some() {
                assert!(!progress.finished(now), "Finished before: {line}");
            }
            progress.on_line(line, now);
        }
        progress
    }

    /// Asserts that the updates are finished exactly `after` the last update-related line
    fn assert_finishes_after(progress: &UpdateProgress, after: Duration) {
        let last_activity = progress.last_activity.unwrap();
        assert!(!progress.finished(last_activity + after - Duration::from_millis(1)));
        assert!(progress.finished(last_activity + after));
    }

    #[test]
    fn multi_patch_updates() {
        // IDEA is updated with 3 patches, DataGrip is updated twice in a row (the second time with
        //  2 patches), and WebStorm and RustRover are updated normally
        let progress = replay(include_str!("../tests/logs/multi-patch.log"));
        assert_eq!(progress.expected_installs(), 5);
        assert_eq!(progress.installs_started, 5);
        assert_finishes_after(&progress, SETTLE_TIME);
    }

    #[test]
    fn already_downloaded_and_chained_updates() {
        // GoLand and WebStorm were downloaded earlier, then WebStorm is updated again with 2 patches
        let progress = replay(include_str!("../tests/logs/chained.log"));
        assert_eq!(progress.expected_installs(), 3);
        assert_eq!(progress.installs_started, 3);
        assert_finishes_after(&progress, SETTLE_TIME);
    }

    #[test]
    fn running_ide() {
        // PyCharm and CLion can't be updated because they're running, until they're closed
        let log = include_str!("../tests/logs/ide-running.log");
        let closed = log
            .find("\n3.8.1.88030 2655730 2026-10-17 20:40:14")
            .unwrap()
            + 1;
        let progress = replay(&log[..closed]);
        assert_eq!(progress.blocked, ["PyCharm", "CLion"]);
        let an_hour = Duration::from_secs(60 * 60);
        assert!(!progress.finished(progress.last_activity.unwrap() + an_hour));

        let progress = replay(log);
        assert_eq!(progress.installs_started, 5);
        assert_finishes_after(&progress, SETTLE_TIME);
    }

    #[test]
    fn major_update() {
        // WebStorm 2025.3 gets a new configuration folder, so it skips the post-install step
        let progress = replay(include_str!("../tests/logs/major-update.log"));
        assert_eq!(progress.installs_started, 2);
        assert_eq!(progress.installs_finished, 1);
        assert_finishes_after(&progress, QUIET_TIME);
    }

    #[test]
    fn self_update() {
        // Never finished: Toolbox restarts itself, which actual_update handles
        let progress = replay(include_str!("../tests/logs/self-update.log"));
        assert!(progress.self_update_pending);
        assert!(progress.verified.is_empty());
        assert!(!progress.finished(progress.last_activity.unwrap() + STALL_TIME));
    }

    #[test]
    fn stalled_download() {
        let now = Instant::now();
        let mut progress = UpdateProgress::default();
        let line = "Downloading from https://download-cdn.jetbrains.com/toolbox/jetbrains-toolbox-3.4.0.77112.tar.gz (offset 0, Content-Range: null)";
        progress.on_line(line, now);
        // A resumed download is logged again, but it's still one download
        let line = "Downloading from https://download-cdn.jetbrains.com/toolbox/jetbrains-toolbox-3.4.0.77112.tar.gz (offset 130284814, Content-Range: bytes 130284814-166483196/166483197)";
        progress.on_line(line, now);
        assert_eq!(progress.downloads.len(), 1);
        assert!(!progress.stalled(now + STALL_TIME - Duration::from_millis(1)));
        assert!(progress.stalled(now + STALL_TIME));
        assert!(!progress.finished(now + STALL_TIME));
    }

    #[test]
    fn ignored_lines() {
        for line in [
            "Downloading from https://downloads.marketplace.jetbrains.com/files/icons/toolbox/io.gitpod.flex/default.svg (offset 0, Content-Range: null)",
            "Moved /home/user/.config/JetBrains/CLion2026.2/3e753f62-8779-45ff-a08c-f1e366b40afc -> /home/user/.config/JetBrains/CLion2026.2/clion64.vmoptions atomically",
            "Moved /home/user/.cache/JetBrains/Toolbox/temp/toolbox-temp-2273730370990171170/3270436c-ff84-460d-955e-f396c08e9ad2 -> /home/user/.cache/JetBrains/Toolbox/temp/toolbox-temp-2273730370990171170/temp64.vmoptions atomically",
            "Notifying `/home/user/.local/share/JetBrains/Toolbox/apps/clion` running at http://127.0.0.1:63343: update-notification, IntelliJApiNotification(...)",
        ] {
            assert_eq!(parse_update_event(line), None, "{line}");
        }
    }

    #[test]
    fn patch_chains() {
        assert_eq!(
            parse_patch("IU-261.24374.151-261.25134.95-patch-unix.jar"),
            Some(("IU", "261.24374.151", "261.25134.95"))
        );
        assert_eq!(parse_patch("idea-2025.3.6.1.tar.gz"), None);
        let progress = UpdateProgress {
            verified: [
                "IU-1.1-1.2-patch-unix.jar",
                "IU-1.2-1.3-patch-unix.jar",
                "WS-1.2-1.3-patch-unix.jar",
                "datagrip-2026.2.4.tar.gz",
            ]
            .map(String::from)
            .to_vec(),
            ..Default::default()
        };
        assert_eq!(progress.expected_installs(), 3);
    }

    #[test]
    fn sleep_unless_interrupted_sleeps() {
        let interrupted = AtomicBool::new(false);
        let start = Instant::now();
        sleep_unless_interrupted(Duration::from_millis(300), &interrupted).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn sleep_unless_interrupted_returns_early() {
        let interrupted = AtomicBool::new(false);
        let start = Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                sleep(Duration::from_millis(200));
                interrupted.store(true, Ordering::Relaxed);
            });
            let result = sleep_unless_interrupted(Duration::from_secs(10), &interrupted);
            assert!(matches!(result, Err(UpdateError::Interrupted)));
        });
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    // All signal assertions are in one test, since signal handlers are process-wide
    //  and tests run in parallel
    #[cfg(unix)]
    #[test]
    fn ctrl_c_guard() {
        use signal_hook::consts::SIGINT;
        use signal_hook::low_level::raise;

        // Stands in for the handler of the application using this library, e.g. Topgrade
        let previous = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(SIGINT, previous.clone()).unwrap();

        let guard = CtrlCGuard::new().unwrap();
        raise(SIGINT).unwrap();
        assert!(guard.interrupted.load(Ordering::Relaxed));
        assert!(previous.load(Ordering::Relaxed));

        let interrupted = guard.interrupted.clone();
        interrupted.store(false, Ordering::Relaxed);
        previous.store(false, Ordering::Relaxed);
        drop(guard);
        raise(SIGINT).unwrap();
        assert!(!interrupted.load(Ordering::Relaxed));
        assert!(previous.load(Ordering::Relaxed));
    }
}
