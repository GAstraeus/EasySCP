//! Runs the actual `scp`, plus the naming rules for the remote file.

use std::path::Path;
use std::process::{Command, Stdio};

/// Remote folder, relative to the remote home directory.
pub const REMOTE_DIR: &str = "screenshots";

/// How the destination is shown to the user (and what "Copy path" copies).
pub fn display_path(remote_name: &str) -> String {
    format!("~/{REMOTE_DIR}/{remote_name}")
}

/// Works out the remote file name.
///
/// * Blank `rename` keeps `original`.
/// * A `rename` without an extension inherits the one from `original`.
/// * When several files share one `rename`, the 2nd, 3rd, ... get `-2`, `-3`, ...
pub fn remote_name(original: &str, rename: &str, index: usize) -> Result<String, String> {
    let rename = rename.trim();
    if rename.is_empty() {
        return Ok(original.to_owned());
    }
    if rename.contains('/') || rename == "." || rename == ".." {
        return Err(format!("\"{rename}\" isn't a valid file name"));
    }

    let (stem, ext) = match split_extension(rename) {
        Some((stem, ext)) => (stem, Some(ext)),
        None => (rename, split_extension(original).map(|(_, ext)| ext)),
    };
    let suffix = if index == 0 { String::new() } else { format!("-{}", index + 1) };
    Ok(match ext {
        Some(ext) => format!("{stem}{suffix}.{ext}"),
        None => format!("{stem}{suffix}"),
    })
}

/// Splits `name` into (stem, extension) if it ends in something that looks
/// like a real extension, so "v1.2 login" isn't mistaken for having one.
fn split_extension(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    let looks_like_ext =
        (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric());
    (!stem.is_empty() && looks_like_ext).then_some((stem, ext))
}

/// Copies `source` to `host:~/screenshots/<remote_name>`. Blocks until done.
pub fn upload(host: &str, source: &Path, remote_name: &str) -> Result<(), String> {
    let first = scp(host, source, remote_name);
    if first.is_ok() {
        return first;
    }
    // Most likely the folder doesn't exist yet. Create it and try once more;
    // if that can't be done either, the original error is the useful one.
    if base_command("ssh")
        .args(["--", host, "mkdir", "-p", REMOTE_DIR])
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return scp(host, source, remote_name);
    }
    first
}

fn scp(host: &str, source: &Path, remote_name: &str) -> Result<(), String> {
    let mut cmd = base_command("scp");
    if source.is_dir() {
        cmd.arg("-r");
    }
    // scp speaks SFTP, so the remote path is taken literally (spaces are
    // fine) and a relative path starts at the remote home directory.
    cmd.arg("--")
        .arg(source)
        .arg(format!("{host}:{REMOTE_DIR}/{remote_name}"));
    let out = cmd.output().map_err(|e| format!("couldn't run scp: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Keep the tail: that's where the actual failure is, after any banners.
    let tail = lines.split_off(lines.len().saturating_sub(3));
    Err(if tail.is_empty() {
        format!("scp failed ({})", out.status)
    } else {
        tail.join("\n")
    })
}

fn base_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    // No terminal to answer prompts, so fail instead of hanging on one.
    cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Apps started from Finder get a bare PATH; ProxyCommand helpers often
    // live in these.
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{path}:/usr/local/bin:/opt/homebrew/bin"));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_rename_keeps_original() {
        assert_eq!(remote_name("Shot 1.png", "", 0).unwrap(), "Shot 1.png");
        assert_eq!(remote_name("Shot 1.png", "   ", 3).unwrap(), "Shot 1.png");
    }

    #[test]
    fn rename_inherits_extension_when_missing() {
        assert_eq!(remote_name("a.png", "login-bug", 0).unwrap(), "login-bug.png");
        assert_eq!(remote_name("a.png", " login-bug ", 0).unwrap(), "login-bug.png");
        assert_eq!(remote_name("a.png", "v1.2 login", 0).unwrap(), "v1.2 login.png");
        assert_eq!(remote_name("Makefile", "build", 0).unwrap(), "build");
        assert_eq!(remote_name("a.tar.gz", "backup", 0).unwrap(), "backup.gz");
    }

    #[test]
    fn rename_with_extension_is_used_as_is() {
        assert_eq!(remote_name("a.png", "final.jpg", 0).unwrap(), "final.jpg");
        assert_eq!(remote_name("a.png", ".hidden", 0).unwrap(), ".hidden.png");
    }

    #[test]
    fn several_files_get_numbered() {
        assert_eq!(remote_name("a.png", "shot", 0).unwrap(), "shot.png");
        assert_eq!(remote_name("b.jpg", "shot", 1).unwrap(), "shot-2.jpg");
        assert_eq!(remote_name("c.png", "shot.png", 2).unwrap(), "shot-3.png");
    }

    #[test]
    fn rejects_paths() {
        assert!(remote_name("a.png", "sub/name", 0).is_err());
        assert!(remote_name("a.png", "..", 0).is_err());
    }
}
