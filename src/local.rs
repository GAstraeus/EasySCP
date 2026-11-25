//! The local side of a download: where it's saved, and showing it afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ssh_config::home_dir;

/// Folder downloads are saved in unless another is chosen, relative to the
/// home directory.
pub const LOCAL_DIR: &str = ".easySCP";

pub fn default_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(LOCAL_DIR))
}

/// How a local path is shown: with `~` for the home directory.
pub fn display(path: &Path) -> String {
    match home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Whether [`pick_folder`] can show a folder chooser here.
pub const CAN_PICK_FOLDER: bool = cfg!(target_os = "macos");

/// Asks for a folder with the system's folder chooser, opened at `start`.
/// `None` if it was cancelled. Call on the main thread; it waits for an answer.
pub fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return mac::pick_folder(start);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = start;
        None
    }
}

/// Shows a downloaded file in the file manager.
pub fn reveal(path: &Path) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut cmd = Command::new("open");
        // Opens the folder with the file selected.
        cmd.arg("-R").arg(path);
        cmd
    } else {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path.parent().unwrap_or(path));
        cmd
    };
    let _ = cmd.status();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};

    use objc2_app_kit::{NSApplication, NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{MainThreadMarker, NSString, NSURL};

    pub fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()?;
        // SAFETY: `mtm` proves this is the main thread, where AppKit panels
        // must be used. Every object passed in outlives the call it's passed to.
        unsafe {
            let window = NSApplication::sharedApplication(mtm).keyWindow();

            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setAllowsMultipleSelection(false);
            panel.setCanCreateDirectories(true);
            panel.setPrompt(Some(&NSString::from_str("Choose")));
            if let Some(start) = start.and_then(Path::to_str) {
                let start = NSURL::fileURLWithPath(&NSString::from_str(start));
                panel.setDirectoryURL(Some(&start));
            }
            let chosen = if panel.runModal() == NSModalResponseOK {
                panel.URL().and_then(|url| url.path())
            } else {
                None
            };

            // Typing goes back to the window the panel was opened from.
            if let Some(window) = window {
                window.makeKeyAndOrderFront(None);
            }
            chosen.map(|path| PathBuf::from(path.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_home_directory_as_a_tilde() {
        let home = home_dir().expect("tests run with HOME set");
        assert_eq!(display(&home), "~");
        assert_eq!(display(&home.join("Downloads/logs")), "~/Downloads/logs");
        assert_eq!(display(Path::new("/var/tmp")), "/var/tmp");
    }
}
