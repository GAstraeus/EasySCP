//! Completing a remote path as it's typed.

/// A file or folder in a remote listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

impl Entry {
    /// The name as it's typed in a path: folders end with a slash.
    pub fn typed(&self) -> String {
        if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Splits a typed path into the folder to list and the start of a name in it:
/// `logs/ap` gives `logs/` and `ap`.
pub fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => path.split_at(slash + 1),
        None => ("", path),
    }
}

/// The last part of a path, which is the name a download gets. `None` when
/// the path doesn't name one thing, like `/` or `~`.
pub fn file_name(path: &str) -> Option<&str> {
    let name = path.trim_end_matches('/').rsplit('/').next()?;
    (!matches!(name, "" | "." | ".." | "~")).then_some(name)
}

/// Reads the output of `ls -1Ap`: one name per line, folders ending in `/`.
pub fn parse_listing(output: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| match line.strip_suffix('/') {
            Some(name) => Entry { name: name.to_owned(), is_dir: true },
            None => Entry { name: line.to_owned(), is_dir: false },
        })
        .collect();
    // `ls` puts capitals first on some servers and not on others.
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

/// The entries a partly typed name could become. Dotfiles only show once a
/// dot is typed. Capitals count unless that leaves nothing.
pub fn matches<'a>(entries: &'a [Entry], partial: &str) -> Vec<&'a Entry> {
    let visible = |entry: &&Entry| partial.starts_with('.') || !entry.name.starts_with('.');
    let exact: Vec<&Entry> = entries
        .iter()
        .filter(visible)
        .filter(|entry| entry.name.starts_with(partial))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let lower = partial.to_lowercase();
    entries
        .iter()
        .filter(visible)
        .filter(|entry| entry.name.to_lowercase().starts_with(&lower))
        .collect()
}

/// What Tab can fill in when several entries match: the start they share.
pub fn common_prefix(matches: &[&Entry]) -> String {
    let Some((first, rest)) = matches.split_first() else {
        return String::new();
    };
    let mut shared = first.name.chars().count();
    for entry in rest {
        let same = first.name.chars().zip(entry.name.chars()).take_while(|(a, b)| a == b);
        shared = shared.min(same.count());
    }
    first.name.chars().take(shared).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.to_owned(), is_dir }
    }

    fn names(matches: &[&Entry]) -> Vec<String> {
        matches.iter().map(|entry| entry.typed()).collect()
    }

    #[test]
    fn splits_at_the_last_slash() {
        assert_eq!(split(""), ("", ""));
        assert_eq!(split("ap"), ("", "ap"));
        assert_eq!(split("~/logs/ap"), ("~/logs/", "ap"));
        assert_eq!(split("/var/log/"), ("/var/log/", ""));
        assert_eq!(split("/"), ("/", ""));
    }

    #[test]
    fn names_the_download() {
        assert_eq!(file_name("~/logs/app.log"), Some("app.log"));
        assert_eq!(file_name("/var/log/"), Some("log"));
        assert_eq!(file_name("notes.txt"), Some("notes.txt"));
        for unnamed in ["", "/", "~", "~/", ".", "logs/..", "//"] {
            assert_eq!(file_name(unnamed), None, "{unnamed:?}");
        }
    }

    #[test]
    fn reads_a_listing() {
        let entries = parse_listing("Zeta.txt\nlogs/\n\n.profile\nmy folder/\napp.log\n");
        assert_eq!(
            entries,
            [
                entry(".profile", false),
                entry("app.log", false),
                entry("logs", true),
                entry("my folder", true),
                entry("Zeta.txt", false),
            ]
        );
    }

    #[test]
    fn matches_what_is_typed() {
        let entries = parse_listing(".cache/\n.profile\nDocuments/\ndata.csv\ndocs/\nnotes.txt\n");

        // Nothing typed: everything but dotfiles.
        assert_eq!(names(&matches(&entries, "")), ["data.csv", "docs/", "Documents/", "notes.txt"]);
        assert_eq!(names(&matches(&entries, ".")), [".cache/", ".profile"]);
        assert_eq!(names(&matches(&entries, "do")), ["docs/"]);
        assert_eq!(names(&matches(&entries, "Do")), ["Documents/"]);
        // No exact match, so capitals stop counting.
        assert_eq!(names(&matches(&entries, "NO")), ["notes.txt"]);
        assert!(matches(&entries, "x").is_empty());
    }

    #[test]
    fn finds_the_shared_start() {
        let entries = parse_listing("app.log\napp.log.1\napple/\nbanana\n");
        assert_eq!(common_prefix(&matches(&entries, "a")), "app");
        assert_eq!(common_prefix(&matches(&entries, "app.")), "app.log");
        assert_eq!(common_prefix(&matches(&entries, "b")), "banana");
        assert_eq!(common_prefix(&[]), "");
    }
}
