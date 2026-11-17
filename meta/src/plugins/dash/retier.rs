//! Applying tier moves: the labels in the workspace config, the Tier column
//! of the workspace's README, and one commit holding both.

use metarepo_core::{MetaConfig, ProjectEntry};
use std::collections::HashMap;
use std::path::Path;

use super::scan::git;
use super::tiers::{self, Move, Tier};

/// The cells of a markdown table row, without the outer pipes. A pipe
/// written `\|` is part of its cell.
fn cells(line: &str) -> Option<Vec<String>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    let mut found = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'|') => {
                found.last_mut()?.push_str("\\|");
                chars.next();
            }
            '|' => found.push(String::new()),
            ch => found.last_mut()?.push(ch),
        }
    }
    Some(found)
}

/// The project a table row is about: its first cell, as the text of a link
/// (`[name](url)`) or as it stands, without code marks.
fn name_in(cell: &str) -> &str {
    let cell = cell.trim();
    let name = cell
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]("))
        .map_or(cell, |(name, _)| name);
    name.trim_matches('`')
}

/// A README with the Tier column of its project tables brought up to
/// `tiers`, or `None` when nothing in it needs to change. A project table is
/// any markdown table with a column headed Tier; a row belongs to the
/// project its first cell names. Rows for projects not in `tiers`, and
/// everything outside such tables, are left exactly as they are.
pub fn sync_readme(text: &str, tiers: &HashMap<String, Tier>) -> Option<String> {
    let mut out = Vec::new();
    let mut changed = false;
    // The Tier column of the table being read, once past its ruler row.
    let mut column: Option<usize> = None;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let Some(row) = cells(line) else {
            column = None;
            out.push(line.to_string());
            continue;
        };
        let Some(at) = column else {
            // A header is a row followed by a ruler (`| --- | --- |`).
            let is_ruler = |next: &&str| {
                cells(next).is_some_and(|ruler| {
                    ruler.iter().all(|cell| {
                        let cell = cell.trim();
                        cell.contains('-') && cell.chars().all(|c| matches!(c, '-' | ':'))
                    })
                })
            };
            let tier_at = row
                .iter()
                .position(|cell| cell.trim().eq_ignore_ascii_case("tier"));
            out.push(line.to_string());
            if let (Some(tier_at), true) = (tier_at, lines.peek().is_some_and(is_ruler)) {
                out.extend(lines.next().map(String::from));
                column = Some(tier_at);
            }
            continue;
        };
        let due = row.first().and_then(|first| tiers.get(name_in(first)));
        match (due, row.get(at)) {
            (Some(tier), Some(now)) if now.trim() != tier.as_str() => {
                let mut row = row.clone();
                row[at] = format!(" {} ", tier.as_str());
                out.push(format!("|{}|", row.join("|")));
                changed = true;
            }
            _ => out.push(line.to_string()),
        }
    }
    changed.then(|| out.join("\n"))
}

/// What applying tier moves came to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Whether the README's Tier column was rewritten.
    pub readme: bool,
    /// The short id of the commit holding the change, when one was made.
    pub commit: Option<String>,
    /// Why there is no commit, when one was asked for.
    pub uncommitted: Option<String>,
}

impl Outcome {
    /// One line for a person: what moved and where that left the workspace.
    pub fn said(&self, moves: &[Move]) -> String {
        let names: Vec<String> = moves
            .iter()
            .map(|one| format!("{} to {}", one.project, one.to.as_str()))
            .collect();
        let tail = match (&self.commit, &self.uncommitted) {
            (Some(id), _) => format!("committed as {id}"),
            (None, Some(why)) => format!("not committed: {why}"),
            (None, None) => "not committed".to_string(),
        };
        format!(
            "retiered {}{} · {tail}",
            names.join(", "),
            if self.readme {
                " · README updated"
            } else {
                ""
            }
        )
    }
}

/// Apply `moves`: rewrite the tier labels in the config, bring the README's
/// Tier column up to every project's tier, and with `commit` make one commit
/// of exactly those two files. Nothing is pushed.
///
/// The commit is skipped, and said why, when the workspace is not a git
/// repository of its own or when either file already had changes: a commit
/// here must hold the retier and nothing a person had under way.
pub fn retier(
    root: &Path,
    config_path: &Path,
    moves: &[Move],
    commit: bool,
) -> Result<Outcome, String> {
    if moves.is_empty() {
        return Ok(Outcome::default());
    }
    let readme = root.join("README.md");
    let config_name = config_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "the workspace config has no name".to_string())?;
    let mut outcome = Outcome::default();
    if commit {
        let is_own_repo = git(root, &["rev-parse", "--show-toplevel"])
            .is_some_and(|top| Path::new(&top).canonicalize().ok() == root.canonicalize().ok());
        if !is_own_repo {
            outcome.uncommitted =
                Some("the workspace is not a git repository of its own".to_string());
        } else if git(
            root,
            &["status", "--porcelain", "--", &config_name, "README.md"],
        )
        .is_none_or(|dirty| !dirty.trim().is_empty())
        {
            outcome.uncommitted = Some(format!("{config_name} or README.md already had changes"));
        }
    }

    tiers::apply(config_path, moves)
        .map_err(|e| format!("could not rewrite {config_name}: {e}"))?;
    // Every project's tier as the config now has it, so a README that had
    // drifted is brought whole, not only the rows that moved today.
    let now: HashMap<String, Tier> = MetaConfig::load_from_file(config_path)
        .map_err(|e| format!("could not read {config_name} back: {e}"))?
        .projects
        .into_iter()
        .filter_map(|(name, entry)| match entry {
            ProjectEntry::Metadata(metadata) => Some((name, Tier::of_labels(&metadata.labels)?)),
            ProjectEntry::Url(_) => None,
        })
        .collect();
    if let Some(synced) = std::fs::read_to_string(&readme)
        .ok()
        .and_then(|text| sync_readme(&text, &now))
    {
        std::fs::write(&readme, synced).map_err(|e| format!("could not write README.md: {e}"))?;
        outcome.readme = true;
    }

    if commit && outcome.uncommitted.is_none() {
        let mut names: Vec<&str> = moves.iter().map(|one| one.project.as_str()).collect();
        names.sort_unstable();
        let message = format!("chore(workspace): retier {}", names.join(", "));
        let mut files = vec![config_name.as_str()];
        if outcome.readme {
            files.push("README.md");
        }
        let mut add = vec!["add", "--"];
        add.extend(&files);
        let mut make = vec!["commit", "-q", "-m", message.as_str(), "--"];
        make.extend(&files);
        if git(root, &add).is_some() && git(root, &make).is_some() {
            outcome.commit = git(root, &["rev-parse", "--short", "HEAD"]);
        } else {
            outcome.uncommitted =
                Some("git would not commit (a hook or signing may have refused)".to_string());
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers(list: &[(&str, Tier)]) -> HashMap<String, Tier> {
        list.iter()
            .map(|(name, tier)| (name.to_string(), *tier))
            .collect()
    }

    const README: &str = "# Workspace\n\nEvery project has a tier: `active` or `dormant`.\n\n### Tools\n| Project | Tier | Description |\n| --- | --- | --- |\n| [claw](https://github.com/org/claw) | active | Agent framework (multi-provider, MCP) |\n| [mind](https://github.com/org/mind) | active | Notes \\| ideas |\n| `plain` | maintained | No link |\n| [gone](https://github.com/org/gone) | dormant | Not in the config |\n\n### Commands\n| Command | Does |\n| --- | --- |\n| `meta status` | active dashboard |\n\nTrailing text with a | pipe.\n";

    #[test]
    fn the_tier_column_follows_the_config_and_nothing_else_moves() {
        let now = tiers(&[
            ("claw", Tier::Active),
            ("mind", Tier::Dormant),
            ("plain", Tier::Active),
        ]);
        let synced = sync_readme(README, &now).unwrap();
        let (before, after): (Vec<&str>, Vec<&str>) =
            (README.lines().collect(), synced.lines().collect());
        assert_eq!(before.len(), after.len());
        let changed: Vec<&str> = after
            .iter()
            .zip(&before)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| *a)
            .collect();
        assert_eq!(
            changed,
            vec![
                // The escaped pipe in a description is still one cell.
                "| [mind](https://github.com/org/mind) | dormant | Notes \\| ideas |",
                "| `plain` | active | No link |",
            ]
        );
        // The final newline, and a table with no Tier column, are untouched.
        assert!(synced.ends_with("pipe.\n"));
        assert!(synced.contains("| `meta status` | active dashboard |"));
    }

    #[test]
    fn a_readme_already_right_is_not_rewritten() {
        let now = tiers(&[
            ("claw", Tier::Active),
            ("mind", Tier::Active),
            ("plain", Tier::Maintained),
        ]);
        assert_eq!(sync_readme(README, &now), None);
        assert_eq!(sync_readme("No tables here.\n", &now), None);
        // A Tier heading with no ruler under it is prose, not a table.
        assert_eq!(
            sync_readme("| Project | Tier |\n| claw | dormant |\n", &now),
            None
        );
    }

    #[test]
    fn the_outcome_is_said_in_one_line() {
        let moves = vec![Move {
            project: "mind".into(),
            from: Tier::Active,
            to: Tier::Dormant,
            quiet_days: 200,
            applied: true,
        }];
        let done = Outcome {
            readme: true,
            commit: Some("abc1234".into()),
            uncommitted: None,
        };
        assert_eq!(
            done.said(&moves),
            "retiered mind to dormant · README updated · committed as abc1234"
        );
        let held = Outcome {
            readme: false,
            commit: None,
            uncommitted: Some(".meta or README.md already had changes".into()),
        };
        assert_eq!(
            held.said(&moves),
            "retiered mind to dormant · not committed: .meta or README.md already had changes"
        );
    }
}
