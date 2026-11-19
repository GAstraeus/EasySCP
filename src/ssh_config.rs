//! Reads host aliases out of `~/.ssh/config` (following `Include` lines).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_INCLUDE_DEPTH: usize = 8;

/// Concrete host aliases from the user's SSH config, in file order.
pub fn load_hosts() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let ssh_dir = home.join(".ssh");
    let mut hosts = Vec::new();
    let mut visited = HashSet::new();
    read_file(&ssh_dir.join("config"), &home, &ssh_dir, &mut hosts, &mut visited, 0);

    let mut seen = HashSet::new();
    hosts.retain(|h| seen.insert(h.clone()));
    hosts
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn read_file(
    path: &Path,
    home: &Path,
    ssh_dir: &Path,
    hosts: &mut Vec<String>,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_INCLUDE_DEPTH || !visited.insert(path.to_path_buf()) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let parsed = parse(&text);
    hosts.extend(parsed.hosts);

    for pattern in parsed.includes {
        let pattern = if let Some(rest) = pattern.strip_prefix("~/") {
            home.join(rest)
        } else if Path::new(&pattern).is_absolute() {
            PathBuf::from(&pattern)
        } else {
            // ssh resolves relative Include paths against ~/.ssh
            ssh_dir.join(&pattern)
        };
        let Some(pattern) = pattern.to_str() else {
            continue;
        };
        let Ok(matches) = glob::glob(pattern) else {
            continue;
        };
        for included in matches.flatten() {
            read_file(&included, home, ssh_dir, hosts, visited, depth + 1);
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct Parsed {
    hosts: Vec<String>,
    includes: Vec<String>,
}

fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // "Keyword args", "Keyword=args" and "Keyword = args" are all valid.
        let split_at = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let keyword = line[..split_at].to_ascii_lowercase();
        let args = line[split_at..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');

        match keyword.as_str() {
            "host" => parsed.hosts.extend(
                split_args(args)
                    .into_iter()
                    // Patterns and negations aren't hosts you can connect to.
                    .filter(|h| !h.contains(['*', '?']) && !h.starts_with('!')),
            ),
            "include" => parsed.includes.extend(split_args(args)),
            _ => {}
        }
    }
    parsed
}

/// Splits on whitespace, honouring double quotes.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in args.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_concrete_hosts_and_skips_patterns() {
        let parsed = parse(
            "Host build-*.example.com\n  User me\n\
             Host web1\n  HostName 10.0.0.1\n\
             host alpha beta !gamma del?a\n\
             Match all\n  ServerAliveInterval 60\n\
             # Host commented\n\
             Host=equals\n\
             Host \"quoted name\"\n",
        );
        assert_eq!(parsed.hosts, ["web1", "alpha", "beta", "equals", "quoted name"]);
        assert!(parsed.includes.is_empty());
    }

    #[test]
    fn collects_includes() {
        let parsed = parse("Include config.d/* ~/other\n  include /abs/path\nHostName host\n");
        assert_eq!(parsed.includes, ["config.d/*", "~/other", "/abs/path"]);
        assert!(parsed.hosts.is_empty());
    }

    #[test]
    fn follows_includes_on_disk() {
        let root = std::env::temp_dir().join(format!("easyscp-test-{}", std::process::id()));
        let ssh = root.join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(ssh.join("config"), "Include config.d/*\nHost main\nHost shared\n").unwrap();
        std::fs::write(ssh.join("config.d/a"), "Host from-include\nHost shared\n").unwrap();
        // A file that includes its parent must not loop forever.
        std::fs::write(ssh.join("config.d/b"), "Include config\n").unwrap();

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        read_file(&ssh.join("config"), &root, &ssh, &mut hosts, &mut visited, 0);
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(hosts, ["main", "shared", "from-include", "shared"]);
    }
}
