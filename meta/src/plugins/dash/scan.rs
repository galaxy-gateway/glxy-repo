//! The local half of the dashboard: each project's state and its worktrees,
//! read from git alone.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::tiers::{tier_due, Tier, QUIET_DAYS};

/// Run `git -C <dir> <args>`; the output without its trailing newline, or
/// `None` when git failed.
pub(super) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Reading must not take the index lock from someone working there.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn number(dir: &Path, args: &[&str]) -> u64 {
    git(dir, args)
        .and_then(|out| out.trim().parse().ok())
        .unwrap_or(0)
}

fn line_count(text: &str) -> u64 {
    text.lines().filter(|line| !line.is_empty()).count() as u64
}

/// `owner/name` for a GitHub remote URL, in any of its spellings
/// (`git@github.com:o/n.git`, `https://github.com/o/n`, `ssh://git@github.com/o/n`).
pub fn github_slug(url: &str) -> Option<String> {
    if !url.contains("github.com") {
        return None;
    }
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let mut parts = url.rsplit(['/', ':']).filter(|part| !part.is_empty());
    let name = parts.next()?;
    let owner = parts.next()?;
    (!owner.contains("github.com")).then(|| format!("{owner}/{name}"))
}

/// The `(path, branch)` of every worktree with a branch checked out, from
/// `git worktree list --porcelain`. The bare entry and detached heads have no
/// branch and are left out.
pub fn parse_worktrees(out: &str) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(p) = path.take() {
                found.push((p, branch.to_string()));
            }
        }
    }
    found
}

/// Whether a project could be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    Ok,
    /// The project directory does not exist.
    Missing,
    /// The directory is not a git repository.
    NotGit,
    /// A bare clone whose `main` worktree is absent.
    NoWorktree,
}

/// One project of the workspace, as read from its clone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    pub name: String,
    pub state: ProjectState,
    pub tier: Option<Tier>,
    /// The tier it belongs in by what was read; differs from `tier` when due a move.
    pub tier_due: Option<Tier>,
    /// Branch checked out in the main worktree.
    pub branch: String,
    /// The remote's default branch where git knows it, else empty.
    pub default_branch: String,
    pub is_default_branch: bool,
    pub ahead: u64,
    pub behind: u64,
    /// Changed working-tree and index entries in the main worktree.
    pub dirty: u64,
    /// Remote branches besides the default.
    pub remote_branches: u64,
    pub bare: bool,
    /// Days since a commit on any branch.
    pub quiet_days: u64,
    /// Work in flight: commits not pushed, worktrees besides the main one, and
    /// other local branches committed to within [`QUIET_DAYS`] days.
    pub work: u64,
    /// `owner/name` on GitHub, when the origin is there.
    pub slug: Option<String>,
}

/// GitHub's summary of a pull request's checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Ci {
    Pass,
    Fail,
    Pending,
    None,
}

/// An open pull request for a branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub is_draft: bool,
    /// GitHub's review decision (`APPROVED`, `CHANGES_REQUESTED`,
    /// `REVIEW_REQUIRED`), or empty.
    pub review: String,
    pub ci: Ci,
}

/// Where a branch stands on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// A session on it is waiting on a person.
    Attention,
    /// Failing checks or changes requested.
    Fix,
    /// A session on it is busy.
    Working,
    /// Uncommitted or unpushed work.
    Local,
    Ready,
    /// A draft, pending checks or a review still required.
    Waiting,
    /// A worktree with nothing in flight and no pull request.
    Idle,
}

/// A branch in flight: a worktree, an unpushed local branch, or an open pull
/// request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub project: String,
    pub branch: String,
    /// Worktree path relative to the workspace root; empty when not checked out.
    pub path: String,
    pub is_default: bool,
    pub dirty: u64,
    pub unpushed: u64,
    pub has_upstream: bool,
    /// Day of the last commit (`YYYY-MM-DD`), else of the pull request's last update.
    pub date: String,
    pub pr: Option<PullRequest>,
    pub section: Section,
}

impl Item {
    pub fn key(&self) -> String {
        format!("{}/{}", self.project, self.branch)
    }

    /// A clean default-branch checkout with no pull request: a project at rest.
    pub fn is_at_rest(&self) -> bool {
        self.is_default && self.pr.is_none() && self.dirty == 0 && self.unpushed == 0
    }
}

fn relative(root: &Path, path: &Path) -> String {
    let within = path
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let real = path.canonicalize().ok()?;
            real.strip_prefix(root).ok().map(Path::to_path_buf)
        });
    within
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Read one project: its state, and every branch of it in flight. `root` is
/// the canonical workspace root, `now` the current unix time.
pub fn scan_project(
    root: &Path,
    name: &str,
    tier: Option<Tier>,
    url: Option<&str>,
    now: u64,
) -> (Project, Vec<Item>) {
    let dir = root.join(name);
    let mut project = Project {
        name: name.to_string(),
        state: ProjectState::Ok,
        tier,
        tier_due: tier,
        branch: String::new(),
        default_branch: String::new(),
        is_default_branch: false,
        ahead: 0,
        behind: 0,
        dirty: 0,
        remote_branches: 0,
        bare: false,
        quiet_days: 0,
        work: 0,
        slug: url.and_then(github_slug),
    };
    if !dir.exists() {
        project.state = ProjectState::Missing;
        return (project, Vec::new());
    }
    if !dir.join(".git").exists() {
        project.state = ProjectState::NotGit;
        return (project, Vec::new());
    }
    if let Some(slug) = git(&dir, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
    {
        project.slug = Some(slug);
    }
    project.bare = git(&dir, &["rev-parse", "--is-bare-repository"]).as_deref() == Some("true");
    project.default_branch = git(
        &dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map(|full| full.trim_start_matches("origin/").to_string())
    .unwrap_or_default();
    let is_default = |branch: &str| {
        if project.default_branch.is_empty() {
            branch == "main" || branch == "master"
        } else {
            branch == project.default_branch
        }
    };

    let last = git(&dir, &["log", "-1", "--all", "--format=%ct"])
        .and_then(|out| out.trim().parse::<u64>().ok())
        .unwrap_or(now);
    project.quiet_days = now.saturating_sub(last) / 86_400;
    project.remote_branches = git(
        &dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ],
    )
    .map(|out| {
        out.lines()
            .filter(|name| {
                !matches!(
                    *name,
                    "" | "origin" | "origin/HEAD" | "origin/main" | "origin/master"
                )
            })
            .count() as u64
    })
    .unwrap_or(0);

    let worktrees = git(&dir, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default();
    // Worktrees besides the one a clone always has: its `main` when bare, itself otherwise.
    let extra = (worktrees.len() as u64).saturating_sub(1);
    let fresh_after = now.saturating_sub(QUIET_DAYS * 86_400);
    let heads = git(
        &dir,
        &[
            "for-each-ref",
            "--format=%(committerdate:unix) %(refname:short)",
            "refs/heads",
        ],
    )
    .unwrap_or_default();
    let fresh = heads
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(when, branch)| {
            !is_default(branch)
                && *branch != "main"
                && *branch != "master"
                && when.parse::<u64>().is_ok_and(|when| when > fresh_after)
        })
        .count() as u64;

    let main = if project.bare {
        dir.join("main")
    } else {
        dir.clone()
    };
    if main.is_dir() {
        project.branch = git(&main, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
        if let Some(counts) = git(
            &main,
            &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
        ) {
            let mut counts = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
            project.behind = counts.next().unwrap_or(0);
            project.ahead = counts.next().unwrap_or(0);
        }
        project.dirty = git(&main, &["status", "--porcelain"])
            .map(|out| line_count(&out))
            .unwrap_or(0);
        project.is_default_branch = is_default(&project.branch);
    } else {
        project.state = ProjectState::NoWorktree;
    }
    project.work = project.ahead + extra + fresh;
    project.tier_due = tier_due(tier, project.work, project.quiet_days);

    let mut items = Vec::new();
    let board_default = if project.default_branch.is_empty() {
        "main"
    } else {
        project.default_branch.as_str()
    };
    for (path, branch) in &worktrees {
        if !path.is_dir() {
            continue;
        }
        let has_upstream = git(path, &["rev-parse", "-q", "--verify", "@{u}"]).is_some();
        let unpushed = if has_upstream {
            number(path, &["rev-list", "--count", "@{u}..HEAD"])
        } else {
            number(path, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        };
        items.push(Item {
            project: name.to_string(),
            branch: branch.clone(),
            path: relative(root, path),
            is_default: branch == board_default,
            dirty: git(path, &["status", "--porcelain"])
                .map(|out| line_count(&out))
                .unwrap_or(0),
            unpushed,
            has_upstream,
            date: git(path, &["log", "-1", "--format=%cs"]).unwrap_or_default(),
            pr: None,
            section: Section::Idle,
        });
    }
    // Local branches not checked out anywhere that hold commits no remote has.
    for branch in heads
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(_, b)| b)
    {
        if branch == board_default
            || worktrees
                .iter()
                .any(|(_, checked_out)| checked_out == branch)
        {
            continue;
        }
        let unpushed = number(&dir, &["rev-list", "--count", branch, "--not", "--remotes"]);
        if unpushed == 0 {
            continue;
        }
        items.push(Item {
            project: name.to_string(),
            branch: branch.to_string(),
            path: String::new(),
            is_default: false,
            dirty: 0,
            unpushed,
            has_upstream: false,
            date: git(&dir, &["log", "-1", "--format=%cs", branch]).unwrap_or_default(),
            pr: None,
            section: Section::Idle,
        });
    }
    (project, items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_from_every_remote_spelling() {
        let want = Some("galaxy-gateway/glxy-repo".to_string());
        assert_eq!(
            github_slug("git@github.com:galaxy-gateway/glxy-repo.git"),
            want
        );
        assert_eq!(
            github_slug("https://github.com/galaxy-gateway/glxy-repo"),
            want
        );
        assert_eq!(
            github_slug("https://github.com/galaxy-gateway/glxy-repo.git/"),
            want
        );
        assert_eq!(
            github_slug("ssh://git@github.com/galaxy-gateway/glxy-repo.git"),
            want
        );
        assert_eq!(github_slug("git@gitlab.com:o/n.git"), None);
        assert_eq!(github_slug("https://github.com/only-owner"), None);
    }

    #[test]
    fn worktrees_with_a_branch_are_listed() {
        let out = "worktree /ws/app\nbare\n\n\
                   worktree /ws/app/main\nHEAD abc\nbranch refs/heads/main\n\n\
                   worktree /ws/app/feat/1-x\nHEAD def\nbranch refs/heads/feat/1-x\n\n\
                   worktree /ws/app/loose\nHEAD 123\ndetached\n";
        assert_eq!(
            parse_worktrees(out),
            vec![
                (PathBuf::from("/ws/app/main"), "main".to_string()),
                (PathBuf::from("/ws/app/feat/1-x"), "feat/1-x".to_string()),
            ]
        );
    }
}
