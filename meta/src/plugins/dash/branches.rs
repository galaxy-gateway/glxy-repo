//! A project's remote branches, sorted by what became of them, and removing
//! the ones whose work is merged. Also finishing a branch: pushing it and
//! opening its pull request, merging that, and clearing up afterwards.
//!
//! Everything here that deletes is held to one rule: a branch goes only
//! while it still points at the commit that was looked at, and each deletion
//! is written down with that commit so it can be put back.

use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::scan::{git, github_slug, parse_worktrees};

/// What became of a remote branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Its commits are in the base branch, or its pull request was merged.
    Merged,
    /// It has pull requests, but none for the commit it points at now.
    Moved,
    /// Its pull request was closed without merging.
    Closed,
    /// No pull request, and commits the base does not have.
    None,
    /// GitHub could not be asked.
    Unknown,
    /// It has an open pull request.
    Open,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Merged => "merged",
            Kind::Moved => "moved on since its pull request",
            Kind::Closed => "closed unmerged",
            Kind::None => "no pull request",
            Kind::Unknown => "unknown",
            Kind::Open => "open",
        }
    }

    /// Whether the dashboard will delete such a branch when asked: only one
    /// whose work is merged, or whose pull request was turned down.
    pub fn is_prunable(self) -> bool {
        matches!(self, Kind::Merged | Kind::Closed)
    }
}

/// A remote branch besides the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    /// The commit it pointed at when it was looked at.
    pub sha: String,
    /// Day of its last commit.
    pub date: String,
    /// Commits it has that the base does not.
    pub ahead: u64,
    pub kind: Kind,
    pub pr: Option<u64>,
}

/// A pull request for a branch, as GitHub lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub oid: String,
    pub state: String,
    pub number: u64,
}

/// A remote branch as git alone knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub name: String,
    pub sha: String,
    pub date: String,
    pub is_ancestor: bool,
    pub ahead: u64,
    pub is_base_tip: bool,
}

/// Sort remote branches by what became of them: the ones safe to remove
/// first. `prs` is every pull request by head branch; `gh_ok` whether GitHub
/// answered at all.
pub fn classify(seen: &[Seen], prs: &HashMap<String, Vec<Pr>>, gh_ok: bool) -> Vec<Branch> {
    let mut found: Vec<Branch> = seen
        .iter()
        .map(|branch| {
            let own = prs.get(&branch.name).map(Vec::as_slice).unwrap_or_default();
            let open = own.iter().find(|pr| pr.state == "OPEN");
            let hit = own.iter().find(|pr| pr.oid == branch.sha);
            let (kind, pr) = if let Some(open) = open {
                (Kind::Open, Some(open.number))
            } else if branch.is_ancestor && (!own.is_empty() || !branch.is_base_tip) {
                // A branch sitting exactly on the base with no pull request may be new work.
                (Kind::Merged, hit.map(|pr| pr.number))
            } else if let Some(hit) = hit {
                let kind = if hit.state == "MERGED" {
                    Kind::Merged
                } else {
                    Kind::Closed
                };
                (kind, Some(hit.number))
            } else if let Some(last) = own.last() {
                (Kind::Moved, Some(last.number))
            } else if gh_ok {
                (Kind::None, None)
            } else {
                (Kind::Unknown, None)
            };
            Branch {
                name: branch.name.clone(),
                sha: branch.sha.clone(),
                date: branch.date.clone(),
                ahead: branch.ahead,
                kind,
                pr,
            }
        })
        .collect();
    found.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    found
}

fn slug_of(dir: &Path) -> Option<String> {
    git(dir, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
}

/// The remote's default branch as `origin/<name>`, else `origin/main`.
fn base_of(dir: &Path) -> String {
    git(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .unwrap_or_else(|| "origin/main".to_string())
}

fn gh(args: &[&str], cwd: Option<&Path>, secs: u64) -> Option<(bool, String)> {
    let mut command = Command::new("gh");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    super::run(command, Duration::from_secs(secs))
}

/// Every remote branch of a project besides its base, sorted by what became
/// of it. Reads what was last fetched; fetch first to see the remote as it is.
pub fn list(root: &Path, project: &str) -> Result<Vec<Branch>, String> {
    let dir = root.join(project);
    if !dir.join(".git").exists() {
        return Err(format!("no project named {project}"));
    }
    let base = base_of(&dir);
    let base_sha = git(&dir, &["rev-parse", "-q", "--verify", &base]).unwrap_or_default();
    let mut prs: HashMap<String, Vec<Pr>> = HashMap::new();
    let listed = slug_of(&dir).and_then(|slug| {
        gh(
            &[
                "pr", "list", "-R", &slug, "--state", "all", "--limit", "500",
            ]
            .into_iter()
            .chain(["--json", "headRefName,headRefOid,state,number"])
            .collect::<Vec<_>>(),
            None,
            60,
        )
        .filter(|(ok, _)| *ok)
        .and_then(|(_, stdout)| serde_json::from_str::<Value>(&stdout).ok())
    });
    let gh_ok = listed.is_some();
    for one in listed
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| {
            one.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        prs.entry(text("headRefName")).or_default().push(Pr {
            oid: text("headRefOid"),
            state: text("state"),
            number: one.get("number").and_then(Value::as_u64).unwrap_or(0),
        });
    }
    let refs = git(
        &dir,
        &[
            "for-each-ref",
            "--format=%(refname:short) %(objectname) %(committerdate:short)",
            "refs/remotes/origin",
        ],
    )
    .unwrap_or_default();
    let base_name = base.trim_start_matches("origin/");
    let seen: Vec<Seen> = refs
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let name = fields.next()?.strip_prefix("origin/")?;
            let (sha, date) = (fields.next()?, fields.next().unwrap_or_default());
            if matches!(name, "HEAD" | "main" | "master") || name == base_name {
                return None;
            }
            Some(Seen {
                name: name.to_string(),
                sha: sha.to_string(),
                date: date.to_string(),
                is_ancestor: git(&dir, &["merge-base", "--is-ancestor", sha, &base]).is_some(),
                ahead: git(&dir, &["rev-list", "--count", &format!("{base}..{sha}")])
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0),
                is_base_tip: sha == base_sha,
            })
        })
        .collect();
    Ok(classify(&seen, &prs, gh_ok))
}

/// Where deletions are written down: one line each, with the commit the
/// branch pointed at, so `git push origin <sha>:refs/heads/<branch>` puts
/// it back.
pub fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("metarepo")
            .join("pruned.log"),
    )
}

fn log_deleted(log: Option<&Path>, project: &str, branch: &str, sha: &str) {
    let Some(log) = log else {
        return;
    };
    if let Some(folder) = log.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let day = jiff::ZonedDateTime::now().date();
        let _ = writeln!(file, "{day} {project} {branch} {sha}");
    }
}

/// Delete a remote branch, but only while the remote still holds exactly
/// `sha` there (`--force-with-lease`): a branch someone pushed to since it
/// was looked at stays. Written to `log` when it goes.
fn delete_remote(dir: &Path, project: &str, branch: &str, sha: &str, log: Option<&Path>) -> bool {
    let mut push = Command::new("git");
    push.arg("-C").arg(dir).args([
        "push",
        &format!("--force-with-lease=refs/heads/{branch}:{sha}"),
        "origin",
        &format!(":refs/heads/{branch}"),
    ]);
    let gone = super::run(push, Duration::from_secs(60)).is_some_and(|(ok, _)| ok);
    if gone {
        log_deleted(log, project, branch, sha);
    }
    gone
}

/// Delete the picked remote branches of a project, each named with the
/// commit it was seen at. Says how it went in one line.
pub fn prune(root: &Path, project: &str, picks: &[(String, String)], log: Option<&Path>) -> String {
    let dir = root.join(project);
    let kept: Vec<&str> = picks
        .iter()
        .filter(|(branch, sha)| !delete_remote(&dir, project, branch, sha, log))
        .map(|(branch, _)| branch.as_str())
        .collect();
    let gone = picks.len() - kept.len();
    let mut said = format!(
        "deleted {gone} remote {} of {project}",
        if gone == 1 { "branch" } else { "branches" }
    );
    if !kept.is_empty() {
        said.push_str(&format!(
            " · kept {} (changed since, or refused)",
            kept.join(", ")
        ));
    }
    if gone > 0 {
        if let Some(log) = log {
            said.push_str(&format!(" · logged in {}", log.display()));
        }
    }
    said
}

/// How a pull request is merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Squash,
    Merge,
    Rebase,
}

impl Method {
    pub const ALL: [Method; 3] = [Method::Squash, Method::Merge, Method::Rebase];

    pub fn word(self) -> &'static str {
        match self {
            Method::Squash => "squash",
            Method::Merge => "merge",
            Method::Rebase => "rebase",
        }
    }
}

/// A way to finish a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    /// Push it and open a pull request, closing `issue` where it names one.
    OpenPr { issue: Option<u64> },
    /// Merge its pull request, then clear up as [`Finish::Cleanup`] does.
    Merge { pr: u64, method: Method },
    /// After a merge: remove the remote branch, the worktree and the local
    /// branch, each only where the merged pull request already holds all of it.
    Cleanup,
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn worktree_of(dir: &Path, branch: &str) -> Option<PathBuf> {
    git(dir, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default()
        .into_iter()
        .find(|(path, checked_out)| checked_out == branch && path.is_dir())
        .map(|(path, _)| path)
}

fn dirty_in(worktree: &Path) -> usize {
    git(worktree, &["status", "--porcelain"])
        .map_or(0, |out| out.lines().filter(|l| !l.is_empty()).count())
}

/// Finish a branch one of three ways. What was done and what was kept, a
/// line each; `Err` where nothing was, with why.
pub fn finish(
    root: &Path,
    project: &str,
    branch: &str,
    how: &Finish,
    log: Option<&Path>,
) -> Result<Vec<String>, String> {
    let dir = root.join(project);
    if !dir.join(".git").exists() {
        return Err(format!("no project named {project}"));
    }
    let base = base_of(&dir);
    let base_name = base.trim_start_matches("origin/").to_string();
    if branch == base_name {
        return Err(format!("{branch} is the default branch"));
    }
    let worktree = worktree_of(&dir, branch);
    let slug = slug_of(&dir);
    let mut did = Vec::new();

    match how {
        Finish::OpenPr { issue } => {
            let worktree =
                worktree.ok_or_else(|| format!("no worktree has {branch} checked out"))?;
            let dirty = dirty_in(&worktree);
            if dirty > 0 {
                return Err(format!(
                    "{dirty} uncommitted files in the worktree: commit or discard them first"
                ));
            }
            let mut fetch = Command::new("git");
            fetch.arg("-C").arg(&dir).args(["fetch", "-q", "origin"]);
            let _ = super::run(fetch, Duration::from_secs(60));
            let ahead: u64 = git(
                &worktree,
                &["rev-list", "--count", &format!("{base}..HEAD")],
            )
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
            if ahead == 0 {
                return Err(format!("{branch} has no commits beyond {base_name}"));
            }
            let mut push = Command::new("git");
            push.arg("-C")
                .arg(&worktree)
                .args(["push", "-q", "-u", "origin", branch]);
            if !super::run(push, Duration::from_secs(120)).is_some_and(|(ok, _)| ok) {
                return Err("the push was refused".to_string());
            }
            did.push(format!("pushed {ahead} commits"));
            let slug = slug.ok_or_else(|| {
                "pushed, but the project is not on GitHub: no pull request".to_string()
            })?;
            let closes = issue.map(|n| format!("Closes #{n}"));
            let mut args = vec![
                "pr", "create", "-R", &slug, "--head", branch, "--base", &base_name, "--fill",
            ];
            if let Some(closes) = &closes {
                args.extend(["--body", closes]);
            }
            match gh(&args, Some(&worktree), 60) {
                Some((true, out)) => did.push(format!("opened {}", last_line(&out))),
                Some((false, out)) if !last_line(&out).is_empty() => {
                    return Err(format!(
                        "pushed, but no pull request was opened: {}",
                        last_line(&out)
                    ))
                }
                _ => return Err("pushed, but no pull request was opened".to_string()),
            }
            return Ok(did);
        }
        Finish::Merge { pr, method } => {
            let slug = slug
                .as_deref()
                .ok_or_else(|| format!("{project} is not on GitHub"))?;
            let number = pr.to_string();
            let now = gh(
                &[
                    "pr",
                    "view",
                    &number,
                    "-R",
                    slug,
                    "--json",
                    "state,headRefName,isDraft",
                ],
                None,
                30,
            )
            .filter(|(ok, _)| *ok)
            .and_then(|(_, out)| serde_json::from_str::<Value>(&out).ok());
            let is_ready = now.as_ref().is_some_and(|now| {
                now.get("state").and_then(Value::as_str) == Some("OPEN")
                    && now.get("headRefName").and_then(Value::as_str) == Some(branch)
                    && now.get("isDraft").and_then(Value::as_bool) == Some(false)
            });
            if !is_ready {
                return Err(format!(
                    "#{pr} is not an open, ready pull request for {branch}"
                ));
            }
            let flag = format!("--{}", method.word());
            match gh(&["pr", "merge", &number, "-R", slug, &flag], None, 120) {
                Some((true, _)) => did.push(format!("merged #{pr}")),
                Some((false, out)) if !last_line(&out).is_empty() => {
                    return Err(format!("merge refused: {}", last_line(&out)))
                }
                _ => return Err("merge refused".to_string()),
            }
        }
        Finish::Cleanup => {}
    }

    // Cleanup, after a merge here or one made elsewhere: only what the merged
    // pull request already holds is removed.
    let none = || format!("no merged pull request for {branch}: nothing was removed");
    let merged = slug
        .as_deref()
        .and_then(|slug| {
            gh(
                &[
                    "pr",
                    "list",
                    "-R",
                    slug,
                    "--head",
                    branch,
                    "--state",
                    "merged",
                    "--json",
                    "number,headRefOid",
                ],
                None,
                30,
            )
        })
        .filter(|(ok, _)| *ok)
        .and_then(|(_, out)| serde_json::from_str::<Value>(&out).ok());
    let sha = merged
        .as_ref()
        .and_then(|list| list.get(0)?.get("headRefOid")?.as_str())
        .filter(|sha| !sha.is_empty())
        .map(String::from);
    let Some(sha) = sha else {
        return if did.is_empty() {
            Err(none())
        } else {
            did.push(none());
            Ok(did)
        };
    };
    let mut on_remote = Command::new("git");
    on_remote
        .arg("-C")
        .arg(&dir)
        .args(["ls-remote", "--exit-code", "--heads", "origin", branch]);
    if super::run(on_remote, Duration::from_secs(30)).is_some_and(|(ok, _)| ok) {
        if delete_remote(&dir, project, branch, &sha, log) {
            did.push("deleted the remote branch".to_string());
        } else {
            did.push(
                "kept the remote branch: it has commits the pull request does not".to_string(),
            );
        }
    }
    let tip = git(
        &dir,
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{branch}"),
        ],
    );
    match (tip, worktree) {
        (Some(tip), _) if tip != sha => {
            did.push(
                "kept the local branch and worktree: they have commits the pull request does not"
                    .to_string(),
            );
        }
        (_, Some(worktree)) => {
            let dirty = dirty_in(&worktree);
            let folder = worktree.to_string_lossy().into_owned();
            if dirty > 0 {
                did.push(format!("kept the worktree: {dirty} uncommitted files"));
            } else if git(&dir, &["worktree", "remove", &folder]).is_some() {
                // A branch like feat/7-x leaves an empty feat folder behind.
                if let Some(parent) = worktree.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
                let _ = git(&dir, &["branch", "-D", branch]);
                did.push("removed the worktree and the local branch".to_string());
            } else {
                did.push("kept the worktree: git would not remove it".to_string());
            }
        }
        (Some(_), None) => {
            if git(&dir, &["branch", "-D", branch]).is_some() {
                did.push("deleted the local branch".to_string());
            }
        }
        (None, None) => {}
    }
    let mut prune = Command::new("git");
    prune
        .arg("-C")
        .arg(&dir)
        .args(["fetch", "-q", "--prune", "origin"]);
    let _ = super::run(prune, Duration::from_secs(60));
    Ok(did)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(name: &str, sha: &str, is_ancestor: bool, is_base_tip: bool) -> Seen {
        Seen {
            name: name.into(),
            sha: sha.into(),
            date: "2026-11-10".into(),
            is_ancestor,
            ahead: if is_ancestor { 0 } else { 2 },
            is_base_tip,
        }
    }

    fn pr(oid: &str, state: &str, number: u64) -> Pr {
        Pr {
            oid: oid.into(),
            state: state.into(),
            number,
        }
    }

    #[test]
    fn branches_are_sorted_by_what_became_of_them() {
        let branches = [
            seen("open", "a1", false, false),
            seen("squashed", "b1", false, false),
            seen("in-base", "c1", true, false),
            seen("fresh", "d1", true, true),
            seen("turned-down", "e1", false, false),
            seen("pushed-after", "f2", false, false),
            seen("plain", "g1", false, false),
        ];
        let prs: HashMap<String, Vec<Pr>> = [
            ("open", vec![pr("zz", "MERGED", 1), pr("a1", "OPEN", 2)]),
            // Squash-merged: not an ancestor of the base, but its pull request for this commit merged.
            ("squashed", vec![pr("b1", "MERGED", 3)]),
            ("turned-down", vec![pr("e1", "CLOSED", 5)]),
            ("pushed-after", vec![pr("f1", "MERGED", 6)]),
        ]
        .into_iter()
        .map(|(name, list)| (name.to_string(), list))
        .collect();
        let found = classify(&branches, &prs, true);
        let told: Vec<(&str, Kind, Option<u64>)> = found
            .iter()
            .map(|b| (b.name.as_str(), b.kind, b.pr))
            .collect();
        assert_eq!(
            told,
            vec![
                ("in-base", Kind::Merged, None),
                ("squashed", Kind::Merged, Some(3)),
                ("pushed-after", Kind::Moved, Some(6)),
                ("turned-down", Kind::Closed, Some(5)),
                // On the base with no pull request: may be work not begun yet.
                ("fresh", Kind::None, None),
                ("plain", Kind::None, None),
                ("open", Kind::Open, Some(2)),
            ]
        );
        assert!(Kind::Merged.is_prunable() && Kind::Closed.is_prunable());
        assert!(
            !Kind::Moved.is_prunable() && !Kind::None.is_prunable() && !Kind::Open.is_prunable()
        );
        // Without GitHub, only what git alone can vouch for is called merged.
        let blind = classify(&branches, &HashMap::new(), false);
        assert_eq!(
            blind
                .iter()
                .filter(|b| b.kind == Kind::Merged)
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            vec!["in-base"]
        );
        assert!(blind
            .iter()
            .filter(|b| b.name != "in-base")
            .all(|b| b.kind == Kind::Unknown));
    }
}
