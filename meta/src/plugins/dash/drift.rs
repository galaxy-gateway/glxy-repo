//! Drift between a workspace and the GitHub account most of it comes from:
//! repositories there that the workspace does not hold, and projects here
//! whose repository there is archived or no longer under that name.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

use super::github::Repo;

/// How the workspace and the account differ. Empty lists mean no drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Drift {
    /// The GitHub organization or user compared against.
    pub org: String,
    /// Repositories there, not archived and not empty, that no project here
    /// comes from.
    pub missing: Vec<String>,
    /// Projects here whose repository there is archived.
    pub archived: Vec<String>,
    /// Projects here with no repository there under that name: renamed,
    /// moved or deleted.
    pub gone: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.archived.is_empty() && self.gone.is_empty()
    }
}

/// A repository as the account lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub is_archived: bool,
    pub is_empty: bool,
}

/// The account a workspace is a view of: the owner of more than half of its
/// GitHub projects, where there are at least two. A workspace gathered from
/// many owners is a view of none, and has no drift to speak of.
pub fn org_of(repos: &[Repo]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for repo in repos {
        if let Some((owner, _)) = repo.slug.split_once('/') {
            *counts.entry(owner.to_lowercase()).or_default() += 1;
        }
    }
    let (owner, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
    (count >= 2 && count * 2 > repos.len()).then_some(owner)
}

/// Compare what `org` lists with the workspace's projects. `own` is the
/// workspace's own repository there, which is not a project of itself.
pub fn compare(org: &str, listed: &[Listed], repos: &[Repo], own: Option<&str>) -> Drift {
    let key = |name: &str| name.to_lowercase();
    // The workspace's projects from this account, by repository name.
    let here: HashMap<String, &str> = repos
        .iter()
        .filter_map(|repo| {
            let (owner, name) = repo.slug.split_once('/')?;
            owner
                .eq_ignore_ascii_case(org)
                .then(|| (key(name), repo.project.as_str()))
        })
        .collect();
    let there: HashMap<String, &Listed> = listed.iter().map(|one| (key(&one.name), one)).collect();
    let mut drift = Drift {
        org: org.to_string(),
        ..Drift::default()
    };
    for one in listed {
        let is_own = own.is_some_and(|own| own.eq_ignore_ascii_case(&one.name));
        if !one.is_archived && !one.is_empty && !is_own && !here.contains_key(&key(&one.name)) {
            drift.missing.push(one.name.clone());
        }
    }
    for (name, project) in &here {
        match there.get(name) {
            Some(one) if one.is_archived => drift.archived.push(project.to_string()),
            Some(_) => {}
            None => drift.gone.push(project.to_string()),
        }
    }
    drift.missing.sort_by_key(|name| name.to_lowercase());
    drift.archived.sort();
    drift.gone.sort();
    drift
}

/// What `gh repo list --json name,isArchived,isEmpty` printed.
pub fn parse_listed(stdout: &str) -> Option<Vec<Listed>> {
    let raw: Value = serde_json::from_str(stdout).ok()?;
    let list = raw
        .as_array()?
        .iter()
        .filter_map(|one| {
            Some(Listed {
                name: one.get("name")?.as_str()?.to_string(),
                is_archived: one
                    .get("isArchived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                is_empty: one.get("isEmpty").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect();
    Some(list)
}

/// Ask GitHub what the workspace's account holds and compare. `None` when
/// the workspace is a view of no one account, or GitHub could not be asked.
pub fn read(repos: &[Repo], own_slug: Option<&str>) -> Option<Drift> {
    let org = org_of(repos)?;
    let mut list = Command::new("gh");
    list.args(["repo", "list", &org, "--limit", "1000"])
        .args(["--json", "name,isArchived,isEmpty"]);
    let (ok, stdout) = super::run(list, Duration::from_secs(60))?;
    let listed = parse_listed(&stdout).filter(|_| ok)?;
    // An account that lists nothing was not read: every project would look gone.
    if listed.is_empty() {
        return None;
    }
    let own = own_slug
        .and_then(|slug| slug.split_once('/'))
        .filter(|(owner, _)| owner.eq_ignore_ascii_case(&org))
        .map(|(_, name)| name);
    Some(compare(&org, &listed, repos, own))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(project: &str, slug: &str) -> Repo {
        Repo {
            project: project.into(),
            slug: slug.into(),
        }
    }

    fn listed(name: &str, is_archived: bool, is_empty: bool) -> Listed {
        Listed {
            name: name.into(),
            is_archived,
            is_empty,
        }
    }

    #[test]
    fn the_account_is_the_owner_of_most_projects() {
        let mostly = [
            repo("a", "Org/a"),
            repo("b", "org/b"),
            repo("c", "someone/c"),
        ];
        assert_eq!(org_of(&mostly).as_deref(), Some("org"));
        // Half and half, or a single project, is a view of no one account.
        assert_eq!(org_of(&[repo("a", "org/a"), repo("c", "someone/c")]), None);
        assert_eq!(org_of(&[repo("a", "org/a")]), None);
        assert_eq!(org_of(&[]), None);
    }

    #[test]
    fn drift_is_what_each_side_has_that_the_other_does_not() {
        let repos = [
            repo("claw", "org/claw"),
            // Kept here under another folder name: matched by its repository.
            repo("terminal", "org/glxy-term"),
            repo("old", "org/old"),
            repo("renamed", "org/before-rename"),
            repo("theirs", "someone/theirs"),
        ];
        let there = [
            listed("claw", false, false),
            listed("Glxy-Term", false, false),
            listed("old", true, false),
            listed("new-thing", false, false),
            listed("workspace", false, false),
            listed("shelved", true, false),
            listed("blank", false, true),
        ];
        let drift = compare("org", &there, &repos, Some("workspace"));
        assert_eq!(drift.org, "org");
        // Not the workspace's own repository, nor archived or empty ones.
        assert_eq!(drift.missing, vec!["new-thing"]);
        assert_eq!(drift.archived, vec!["old"]);
        // Another owner's project is not this account's to list.
        assert_eq!(drift.gone, vec!["renamed"]);
        assert!(!drift.is_empty());

        let same = compare("org", &there[..2], &repos[..2], None);
        assert!(same.is_empty());
    }

    #[test]
    fn the_list_is_read_from_what_gh_printed() {
        let out = r#"[{"name":"claw","isArchived":false,"isEmpty":false},{"name":"old","isArchived":true},{"nope":1}]"#;
        assert_eq!(
            parse_listed(out),
            Some(vec![
                listed("claw", false, false),
                listed("old", true, false)
            ])
        );
        assert_eq!(parse_listed("not json"), None);
        assert_eq!(parse_listed("{}"), None);
    }
}
