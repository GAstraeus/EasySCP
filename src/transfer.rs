//! Runs the actual `scp`.

use std::path::Path;
use std::process::{Command, Stdio};

/// Remote folder, relative to the remote home directory.
pub const REMOTE_DIR: &str = "screenshots";

/// How the destination is shown to the user (and what "Copy path" copies).
pub fn display_path(remote_name: &str) -> String {
    format!("~/{REMOTE_DIR}/{remote_name}")
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
