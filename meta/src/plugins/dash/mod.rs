//! `meta dash`: one read of everything a workspace dashboard shows.
//!
//! This is the data half, with no interface of its own: each project's git
//! state and tier, the branches in flight with their pull requests and checks,
//! open issues, and the Claude Code sessions running in the workspace. It is
//! ported from the `/metarepo` dashboard mod (galaxy-gateway/metarepo-dashboard)
//! and printed by `meta dash --json`.

use anyhow::{anyhow, Result};
use metarepo_core::{ProjectEntry, RuntimeConfig};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod actions;
pub mod branches;
mod cost;
mod drift;
mod github;
pub mod host;
mod plugin;
pub mod retier;
mod scan;
mod sessions;
pub mod start;
mod tiers;
mod tui;
mod view;
mod workspaces;

pub use cost::{Cost, CostRow, SessionCost};
pub use drift::Drift;
pub use github::Issue;
pub use plugin::DashPlugin;
pub use scan::{Ci, Item, Project, ProjectState, PullRequest, Section};
pub use sessions::{Place, Session};
pub use tiers::{Move, Tier};

/// Version of the JSON this prints. Raised when a field changes meaning or goes.
pub const SCHEMA: u32 = 1;

/// Run a command with nothing on stdin, for at most `timeout`. Whether it
/// succeeded and what it printed; `None` when it could not start or ran out
/// of time (it is killed).
fn run(mut command: Command, timeout: Duration) -> Option<(bool, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = reader.join().ok()?;
    status.map(|status| (status.success(), stdout))
}

/// What a read should include.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Ask GitHub for pull requests and issues (needs the `gh` CLI, logged in).
    pub github: bool,
    /// List Claude Code sessions (needs the `claude` CLI).
    pub sessions: bool,
    /// Rewrite the workspace config for projects whose tier is due a move.
    pub retier: bool,
    /// Add up what Claude Code has cost from its session transcripts.
    pub cost: bool,
    /// With `retier`: commit the config and README change it makes.
    pub commit: bool,
}

/// What became of the GitHub half of a read.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GitHub {
    /// Whether GitHub was asked at all.
    pub asked: bool,
    /// Projects GitHub gave no pull-request or issue answer for.
    pub failed: Vec<String>,
}

/// One read of the workspace.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub schema: u32,
    /// Absolute path of the workspace root.
    pub root: String,
    /// When it was read, in unix seconds.
    pub at: u64,
    /// Whether any project carries a tier label.
    pub has_tiers: bool,
    pub projects: Vec<Project>,
    /// Branches in flight, by project then branch.
    pub board: Vec<Item>,
    /// Projects with nothing on the board.
    pub at_rest: u64,
    /// Open issues, newest first.
    pub issues: Vec<Issue>,
    pub sessions: Vec<Session>,
    /// What Claude Code has cost, from its transcripts; `None` when not read.
    pub cost: Option<Cost>,
    /// Projects whose tier label no longer matches what was read of them.
    pub retier: Vec<Move>,
    /// How the workspace differs from the GitHub account most of it comes
    /// from; `None` when GitHub was not asked or it is a view of no one account.
    pub drift: Option<Drift>,
    pub github: GitHub,
    /// Things that could not be read, in plain words.
    pub notes: Vec<String>,
}

/// Where a branch stands on the board, given the sessions working on it.
pub fn section_of(item: &Item, own: &[&Session]) -> Section {
    let pr = item.pr.as_ref();
    if own.iter().any(|session| session.needs_you) {
        Section::Attention
    } else if pr.is_some_and(|pr| pr.ci == Ci::Fail || pr.review == "CHANGES_REQUESTED") {
        Section::Fix
    } else if own.iter().any(|session| session.status == "busy") {
        Section::Working
    } else if item.dirty > 0 || item.unpushed > 0 {
        Section::Local
    } else {
        match pr {
            None => Section::Idle,
            Some(pr) if pr.is_draft || pr.ci == Ci::Pending || pr.review == "REVIEW_REQUIRED" => {
                Section::Waiting
            }
            Some(_) => Section::Ready,
        }
    }
}

/// Where GitHub gave no answer for a project this time, keep what was last
/// seen of it: its pull requests on the board and its issues. Without this a
/// timeout would strip a project's rows until the next read that works.
pub fn keep_last_seen(new: &mut Snapshot, old: &Snapshot) {
    if new.github.failed.is_empty() {
        return;
    }
    let failed: HashSet<&str> = new.github.failed.iter().map(String::as_str).collect();
    for was in old
        .board
        .iter()
        .filter(|item| item.pr.is_some() && failed.contains(item.project.as_str()))
    {
        match new
            .board
            .iter_mut()
            .find(|item| item.project == was.project && item.branch == was.branch)
        {
            Some(item) => item.pr = was.pr.clone(),
            // Not checked out here: the row is its pull request and nothing else.
            None if was.path.is_empty() => new.board.push(was.clone()),
            // It had a worktree and has none now: the branch is gone locally.
            None => {}
        }
    }
    new.board
        .sort_by(|a, b| a.project.cmp(&b.project).then(a.branch.cmp(&b.branch)));
    new.issues.extend(
        old.issues
            .iter()
            .filter(|issue| failed.contains(issue.project.as_str()))
            .cloned(),
    );
    new.issues.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(a.project.cmp(&b.project))
            .then(b.number.cmp(&a.number))
    });
    place_sessions(&mut new.board, &new.sessions);
    github::mark_in_flight(&mut new.issues, &new.board);
    let busy: HashSet<&str> = new.board.iter().map(|item| item.project.as_str()).collect();
    new.at_rest = new
        .projects
        .iter()
        .filter(|project| {
            project.state == ProjectState::Ok && !busy.contains(project.name.as_str())
        })
        .count() as u64;
}

/// Give each branch on the board its section, by the sessions at work on it.
/// Run again whenever the sessions are read anew.
pub fn place_sessions(board: &mut [Item], sessions: &[Session]) {
    let mut own: HashMap<String, Vec<&Session>> = HashMap::new();
    for session in sessions {
        if let Some(item) = sessions::item_of(session, board) {
            own.entry(item.key()).or_default().push(session);
        }
    }
    let sections: Vec<Section> = board
        .iter()
        .map(|item| section_of(item, own.get(&item.key()).map_or(&[], Vec::as_slice)))
        .collect();
    for (item, section) in board.iter_mut().zip(sections) {
        item.section = section;
    }
}

fn tier_in_config(config: &RuntimeConfig, key: &str) -> Option<Tier> {
    match config.meta_config.projects.get(key)? {
        ProjectEntry::Metadata(metadata) => Tier::of_labels(&metadata.labels),
        ProjectEntry::Url(_) => None,
    }
}

/// Read the whole workspace `config` belongs to, whatever directory the
/// command ran from. Disabled projects, and those ruled out by `--label` or
/// `--exclude-label`, are left out.
pub fn gather(config: &RuntimeConfig, options: Options) -> Result<Snapshot> {
    let root = config
        .meta_root()
        .ok_or_else(|| anyhow!("No workspace config found. Run 'meta init' first."))?;
    let root = root.canonicalize().unwrap_or(root);
    let keys = metarepo_core::scoped_keys(
        &config.meta_config,
        &config.working_dir,
        config.meta_file_path.as_deref(),
        true,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let mut notes = Vec::new();

    // Each project is a dozen short git calls: read several at once.
    let jobs = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let per_job = keys.len().div_ceil(jobs).max(1);
    let mut projects = Vec::with_capacity(keys.len());
    let mut board = Vec::new();
    std::thread::scope(|scope| {
        let reads: Vec<_> = keys
            .chunks(per_job)
            .map(|chunk| {
                let root = &root;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|key| {
                            let url = config.meta_config.get_project_url(key);
                            scan::scan_project(
                                root,
                                key,
                                tier_in_config(config, key),
                                url.as_deref(),
                                now,
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for read in reads {
            for (project, items) in read.join().unwrap_or_default() {
                projects.push(project);
                board.extend(items);
            }
        }
    });

    let repos: Vec<github::Repo> = projects
        .iter()
        .filter(|project| project.state != ProjectState::Missing)
        .filter_map(|project| {
            Some(github::Repo {
                project: project.name.clone(),
                slug: project.slug.clone()?,
            })
        })
        .collect();
    let slugs: HashMap<String, String> = repos
        .iter()
        .map(|repo| (repo.slug.to_lowercase(), repo.project.clone()))
        .collect();

    // GitHub, the session list and the transcripts do not depend on each other:
    // read together.
    let own_slug = scan::git(&root, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(scan::github_slug);
    let (prs, issues, sessions, cost, drift) = std::thread::scope(|scope| {
        let prs = scope.spawn(|| options.github.then(|| github::read_prs(&repos)));
        let issues = scope.spawn(|| options.github.then(|| github::read_issues(&repos)));
        let sessions = scope.spawn(|| {
            options
                .sessions
                .then(|| sessions::read(&root, &keys, &slugs))
        });
        let cost = scope.spawn(|| options.cost.then(|| cost::read(&root, &keys, &slugs)));
        let drift = scope.spawn(|| {
            options
                .github
                .then(|| drift::read(&repos, own_slug.as_deref()))
        });
        (
            prs.join().ok().flatten(),
            issues.join().ok().flatten(),
            sessions.join().ok().flatten(),
            cost.join().ok().flatten().flatten(),
            drift.join().ok().flatten().flatten(),
        )
    });

    let mut failed: HashSet<String> = HashSet::new();
    if let Some((by_project, gone)) = prs {
        failed.extend(gone);
        for (project, list) in by_project {
            for found in list {
                let at = board
                    .iter()
                    .position(|item| item.project == project && item.branch == found.branch);
                let item = match at {
                    Some(at) => &mut board[at],
                    None => {
                        board.push(Item {
                            project: project.clone(),
                            branch: found.branch.clone(),
                            path: String::new(),
                            is_default: false,
                            dirty: 0,
                            unpushed: 0,
                            has_upstream: true,
                            date: String::new(),
                            pr: None,
                            section: Section::Idle,
                        });
                        board.last_mut().expect("just pushed")
                    }
                };
                if item.date.is_empty() {
                    item.date = found.updated;
                }
                item.pr = Some(found.pr);
            }
        }
    }
    let mut issues = match issues {
        Some((issues, gone)) => {
            failed.extend(gone);
            issues
        }
        None => Vec::new(),
    };
    if options.github && !repos.is_empty() && failed.len() == repos.len() {
        notes.push("GitHub gave no answer: is the gh CLI installed and logged in?".to_string());
    }
    let sessions = match sessions {
        Some(Some(sessions)) => sessions,
        Some(None) => {
            notes.push(
                "Claude sessions could not be listed: is the claude CLI installed?".to_string(),
            );
            Vec::new()
        }
        None => Vec::new(),
    };

    board.retain(|item| !item.is_at_rest());
    board.sort_by(|a, b| a.project.cmp(&b.project).then(a.branch.cmp(&b.branch)));
    place_sessions(&mut board, &sessions);
    github::mark_in_flight(&mut issues, &board);
    let busy: HashSet<&str> = board.iter().map(|item| item.project.as_str()).collect();
    let at_rest = projects
        .iter()
        .filter(|project| {
            project.state == ProjectState::Ok && !busy.contains(project.name.as_str())
        })
        .count() as u64;

    let mut retier: Vec<Move> = projects
        .iter()
        .filter_map(|project| {
            let (from, to) = (project.tier?, project.tier_due?);
            (from != to && project.state == ProjectState::Ok).then(|| Move {
                project: project.name.clone(),
                from,
                to,
                quiet_days: project.quiet_days,
                applied: false,
            })
        })
        .collect();
    if options.retier && !retier.is_empty() {
        let path = config
            .meta_file_path
            .as_ref()
            .ok_or_else(|| anyhow!("No workspace config found. Run 'meta init' first."))?;
        let outcome =
            retier::retier(&root, path, &retier, options.commit).map_err(|why| anyhow!(why))?;
        notes.push(outcome.said(&retier));
        for one in &mut retier {
            one.applied = true;
            if let Some(project) = projects.iter_mut().find(|p| p.name == one.project) {
                project.tier = Some(one.to);
            }
        }
    }

    let mut failed: Vec<String> = failed.into_iter().collect();
    failed.sort();
    Ok(Snapshot {
        schema: SCHEMA,
        root: root.to_string_lossy().into_owned(),
        at: now,
        has_tiers: projects.iter().any(|project| project.tier.is_some()),
        projects,
        board,
        at_rest,
        issues,
        sessions,
        cost,
        retier,
        drift,
        github: GitHub {
            asked: options.github,
            failed,
        },
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(dirty: u64, unpushed: u64, pr: Option<PullRequest>) -> Item {
        Item {
            project: "app".into(),
            branch: "feat/1-x".into(),
            path: "app/feat/1-x".into(),
            is_default: false,
            dirty,
            unpushed,
            has_upstream: true,
            date: String::new(),
            pr,
            section: Section::Idle,
        }
    }

    fn pr(is_draft: bool, review: &str, ci: Ci) -> Option<PullRequest> {
        Some(PullRequest {
            number: 1,
            title: String::new(),
            is_draft,
            review: review.into(),
            ci,
        })
    }

    fn session(status: &str) -> Session {
        Session {
            id: "a".into(),
            session_id: "a-0".into(),
            name: "a".into(),
            kind: "background".into(),
            status: status.into(),
            needs_you: sessions::needs_you(status),
            started_at: 0,
            cwd: String::new(),
            project: "app".into(),
            branch: "feat/1-x".into(),
            place: Place::In,
        }
    }

    #[test]
    fn a_branch_is_sorted_by_what_it_most_needs() {
        let (asking, busy) = (session("permission"), session("busy"));
        // A person is asked for: that comes before failing checks.
        assert_eq!(
            section_of(&item(0, 0, pr(false, "", Ci::Fail)), &[&asking]),
            Section::Attention
        );
        // Failing checks come before an agent at work on them.
        assert_eq!(
            section_of(&item(0, 0, pr(false, "", Ci::Fail)), &[&busy]),
            Section::Fix
        );
        assert_eq!(
            section_of(&item(0, 0, pr(false, "CHANGES_REQUESTED", Ci::Pass)), &[]),
            Section::Fix
        );
        assert_eq!(
            section_of(&item(2, 0, pr(false, "", Ci::Pass)), &[&busy]),
            Section::Working
        );
        assert_eq!(
            section_of(&item(0, 1, pr(false, "", Ci::Pass)), &[]),
            Section::Local
        );
        assert_eq!(section_of(&item(0, 0, None), &[]), Section::Idle);
        assert_eq!(
            section_of(&item(0, 0, pr(true, "", Ci::Pass)), &[]),
            Section::Waiting
        );
        assert_eq!(
            section_of(&item(0, 0, pr(false, "", Ci::Pending)), &[]),
            Section::Waiting
        );
        assert_eq!(
            section_of(&item(0, 0, pr(false, "REVIEW_REQUIRED", Ci::Pass)), &[]),
            Section::Waiting
        );
        assert_eq!(
            section_of(&item(0, 0, pr(false, "APPROVED", Ci::Pass)), &[]),
            Section::Ready
        );
    }

    #[test]
    fn a_command_that_outlives_its_time_is_stopped() {
        let mut slow = Command::new("sleep");
        slow.arg("5");
        let started = Instant::now();
        assert!(run(slow, Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
        let mut quick = Command::new("git");
        quick.arg("--version");
        let (ok, out) = run(quick, Duration::from_secs(10)).unwrap();
        assert!(ok && out.starts_with("git version"));
    }
}
