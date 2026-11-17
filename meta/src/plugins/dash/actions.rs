//! Things the dashboard does to the whole workspace, or fetches on request:
//! fetching every project, syncing them, and reading an issue's text.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::scan::{git, github_slug};

/// Fetch every project from its origin, pruning branches gone there, several
/// at a time. Says how it went in one line. Nothing in a working tree moves.
pub fn fetch_all(root: &Path, keys: &[String]) -> String {
    let repos: Vec<&String> = keys
        .iter()
        .filter(|key| root.join(key).join(".git").exists())
        .collect();
    let per_job = repos.len().div_ceil(16).max(1);
    let mut failed: Vec<String> = Vec::new();
    std::thread::scope(|scope| {
        let jobs: Vec<_> = repos
            .chunks(per_job)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter(|key| {
                            let mut fetch = Command::new("git");
                            fetch
                                .arg("-C")
                                .arg(root.join(key.as_str()))
                                .args(["fetch", "--prune", "-q"]);
                            !super::run(fetch, Duration::from_secs(90)).is_some_and(|(ok, _)| ok)
                        })
                        .map(|key| key.to_string())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for job in jobs {
            failed.extend(job.join().unwrap_or_default());
        }
    });
    failed.sort();
    if failed.is_empty() {
        format!("fetched {} projects", repos.len())
    } else {
        format!(
            "fetched {} of {} projects · could not fetch {}",
            repos.len() - failed.len(),
            repos.len(),
            failed.join(", ")
        )
    }
}

/// Run `meta git sync` for the workspace: clone what is missing, then pull
/// every project. `meta` is the binary to run. Says how it ended in one line.
pub fn sync_all(meta: &Path, root: &Path) -> String {
    let mut sync = Command::new(meta);
    sync.current_dir(root)
        .args(["--non-interactive", "defaults", "git", "sync"]);
    match super::run(sync, Duration::from_secs(600)) {
        Some((ok, stdout)) => {
            let last = stdout
                .lines()
                .rev()
                .map(strip_ansi)
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default();
            let last = last.trim();
            match (ok, last.is_empty()) {
                (true, true) => "synced every project".to_string(),
                (true, false) => format!("synced: {last}"),
                (false, true) => "meta git sync failed".to_string(),
                (false, false) => format!("meta git sync failed: {last}"),
            }
        }
        None => "meta git sync could not be run, or did not finish in ten minutes".to_string(),
    }
}

/// A line without the colour and cursor sequences a command wrote into it.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // Parameters, then one final letter.
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// Text as plain lines for a pane: no tabs, no carriage returns, no runs of
/// blank lines, and none at either end.
pub fn tidy(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.replace('\r', "").replace('\t', "  ");
        let line = line.trim_end();
        if line.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(line.to_string());
    }
    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines
}

/// An issue's byline and body, asked of GitHub through `gh`.
pub fn issue_text(root: &Path, project: &str, number: u64) -> Result<Vec<String>, String> {
    let slug = git(&root.join(project), &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
        .ok_or_else(|| format!("{project} is not on GitHub"))?;
    let mut view = Command::new("gh");
    view.args(["issue", "view", &number.to_string(), "-R", &slug])
        .args(["--json", "body,author,createdAt,comments", "-q"])
        .arg(r#""by " + .author.login + " · opened " + .createdAt[0:10] + " · " + (.comments|length|tostring) + " comments", "", .body"#);
    let text = super::run(view, Duration::from_secs(30))
        .filter(|(ok, _)| *ok)
        .map(|(_, stdout)| stdout)
        .ok_or_else(|| format!("could not load #{number} of {slug}"))?;
    let mut lines = tidy(&text);
    if lines.len() <= 1 {
        lines.push(String::new());
        lines.push("(the issue has no description)".to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_tidied_for_a_pane() {
        assert_eq!(
            tidy("\n\nby someone\r\n\n\n\tindented\ttab  \nlast\n\n\n"),
            vec!["by someone", "", "  indented  tab", "last"]
        );
        assert!(tidy("\n \n").is_empty());
    }

    #[test]
    fn colour_is_taken_out_of_a_commands_last_line() {
        assert_eq!(
            strip_ansi("\u{1b}[1;32m✓\u{1b}[0m 3 pulled, \u{1b}[2K1 failed"),
            "✓ 3 pulled, 1 failed"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn fetching_says_what_could_not_be_fetched() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Not a repository: not counted. A repository with no origin: it fails.
        std::fs::create_dir_all(tmp.path().join("plain")).unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        if git(&repo, &["init", "-q"]).is_none() {
            return;
        }
        let said = fetch_all(
            tmp.path(),
            &["app".to_string(), "plain".to_string(), "absent".to_string()],
        );
        // A repository with no remote fetches nothing and git calls that success.
        assert_eq!(said, "fetched 1 projects");
        git(&repo, &["remote", "add", "origin", "/nowhere/at/all.git"]);
        let said = fetch_all(tmp.path(), &["app".to_string()]);
        assert_eq!(said, "fetched 0 of 1 projects · could not fetch app");
    }
}
