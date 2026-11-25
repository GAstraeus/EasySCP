//! Runs the actual `scp` and `ssh`.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Remote folder uploads go to, relative to the remote home directory.
pub const REMOTE_DIR: &str = ".easySCP";

/// Listing a folder for each step of a typed path would log in again every
/// time, so the first listing leaves its connection open for a minute and the
/// next ones ride on it.
const SHARED_CONNECTION: [&str; 6] = [
    "-o",
    "ControlMaster=auto",
    "-o",
    "ControlPath=~/.ssh/easyscp-%C",
    "-o",
    "ControlPersist=60",
];

/// How the destination is shown to the user (and what "Copy path" copies).
pub fn display_path(remote_name: &str) -> String {
    format!("~/{REMOTE_DIR}/{remote_name}")
}

/// Copies `source` to `host:~/.easySCP/<remote_name>`. Blocks until done.
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
    run_scp(cmd)
}

/// Copies `host:<remote_path>` into `local_dir` under its own name. A folder
/// comes with everything in it. Blocks until done.
pub fn download(host: &str, remote_path: &str, local_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(local_dir)
        .map_err(|e| format!("couldn't create {}: {e}", local_dir.display()))?;
    let mut cmd = base_command("scp");
    // -r lets the path be a folder, and does nothing to a file.
    cmd.args(["-r", "--"])
        .arg(format!("{host}:{}", escape_for_scp(home_relative(remote_path))))
        .arg(local_dir);
    run_scp(cmd)
}

fn run_scp(mut cmd: Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("couldn't run scp: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(failure("scp", &out))
    }
}

/// Lists the remote folder `dir` as `ls -1Ap` prints it: one name per line,
/// folders ending in `/`. Blocks until done.
pub fn list(host: &str, dir: &str) -> Result<String, String> {
    let dir = match home_relative(dir) {
        "" => ".",
        dir => dir,
    };
    let mut cmd = base_command("ssh");
    cmd.args(SHARED_CONNECTION).stdout(Stdio::piped());
    // `exec` skips any `ls` alias or function on the server. -L shows a link
    // to a folder as the folder it leads to.
    cmd.args(["--", host])
        .arg(format!("exec ls -1ApL -- {}", shell_quote(dir)));
    let out = cmd.output().map_err(|e| format!("couldn't run ssh: {e}"))?;
    let names = String::from_utf8_lossy(&out.stdout);
    // `ls` reports failure when it can't read one entry, but still lists the
    // rest, so go by what it printed.
    if out.status.success() || !names.trim().is_empty() {
        Ok(names.into_owned())
    } else {
        Err(failure("ssh", &out))
    }
}

/// A typed remote path as `ssh` and `scp` take it. Their relative paths
/// already start at the remote home directory, so `~/` is dropped.
fn home_relative(path: &str) -> &str {
    if path == "~" {
        return "";
    }
    match path.strip_prefix("~/") {
        // `~//etc` is still inside the home directory.
        Some(rest) => rest.trim_start_matches('/'),
        None => path,
    }
}

/// Makes `scp` take a remote path literally. It treats `*`, `?` and `[` in a
/// path to download as wildcards. A backslash turns that off, and also keeps
/// the path whole in the remote shell that older versions of scp send it to.
fn escape_for_scp(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii() && !c.is_ascii_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Quotes `text` as one word for the remote shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Why `program` failed, in its own words.
fn failure(program: &str, out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Keep the tail: that's where the actual failure is, after any banners.
    let tail = lines.split_off(lines.len().saturating_sub(3));
    if tail.is_empty() {
        format!("{program} failed ({})", out.status)
    } else {
        tail.join("\n")
    }
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
    fn drops_the_home_prefix() {
        assert_eq!(home_relative("~"), "");
        assert_eq!(home_relative("~/"), "");
        assert_eq!(home_relative("~/logs/app.log"), "logs/app.log");
        assert_eq!(home_relative("~//etc"), "etc");
        assert_eq!(home_relative("logs/"), "logs/");
        assert_eq!(home_relative("/var/log"), "/var/log");
        // Not the home directory: a file whose name starts with a tilde.
        assert_eq!(home_relative("~backup"), "~backup");
    }

    #[test]
    fn escapes_what_scp_would_expand() {
        assert_eq!(escape_for_scp("logs/app-1_final.log"), "logs/app-1_final.log");
        assert_eq!(escape_for_scp("my file [1]*.txt"), r"my\ file\ \[1\]\*.txt");
        assert_eq!(escape_for_scp(r"a\b"), r"a\\b");
        assert_eq!(escape_for_scp("~backup"), r"\~backup");
        assert_eq!(escape_for_scp("naïve.txt"), "naïve.txt");
    }

    #[test]
    fn quotes_for_the_remote_shell() {
        assert_eq!(shell_quote("my folder"), "'my folder'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$HOME;ls"), "'$HOME;ls'");
    }
}
