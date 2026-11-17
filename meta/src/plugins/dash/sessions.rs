//! Claude Code sessions on this machine, placed against the workspace.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use super::scan::{git, github_slug, Item};

const TIMEOUT: Duration = Duration::from_secs(20);

/// Where a session's folder is, relative to the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// The workspace root itself.
    Root,
    /// Inside the workspace.
    In,
    /// Another checkout of one of its projects, matched by remote.
    Out,
    None,
}

/// A Claude Code session, as `claude agents --json --all` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    /// Short id, the one `claude attach` takes.
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub kind: String,
    /// `busy`, `idle`, `done`, or whatever it is stopped on.
    pub status: String,
    /// Whether it is stopped on something only a person can answer.
    pub needs_you: bool,
    pub started_at: u64,
    pub cwd: String,
    /// The workspace project it runs in, or empty.
    pub project: String,
    /// The branch checked out in its folder, or empty.
    pub branch: String,
    #[serde(rename = "where")]
    pub place: Place,
}

/// A session is busy, idle, done, or stopped on something only a person can
/// answer: a permission, a question. Whatever is none of the first three.
pub fn needs_you(status: &str) -> bool {
    !matches!(status, "busy" | "idle" | "done")
}

/// The sessions in what `claude agents --json --all` printed, not yet placed.
/// `None` when it is not the list expected.
pub fn parse_agents(stdout: &str) -> Option<Vec<Session>> {
    let raw: Value = serde_json::from_str(stdout).ok()?;
    let text = |one: &Value, key: &str| -> String {
        one.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let list = raw
        .as_array()?
        .iter()
        .filter(|one| one.is_object())
        .filter_map(|one| {
            let session_id = text(one, "sessionId");
            let short: String = session_id.chars().take(8).collect();
            let id = Some(text(one, "id"))
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| short.clone());
            if id.is_empty() {
                return None;
            }
            // A background session that ran to its end reads idle, state done.
            let status = if text(one, "state") == "done" {
                "done".to_string()
            } else {
                Some(text(one, "status"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "idle".to_string())
            };
            Some(Session {
                id,
                name: Some(text(one, "name"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or(short),
                kind: text(one, "kind"),
                needs_you: needs_you(&status),
                status,
                started_at: one
                    .get("startedAt")
                    .and_then(|at| at.as_u64().or_else(|| at.as_f64().map(|at| at as u64)))
                    .unwrap_or(0),
                cwd: text(one, "cwd"),
                session_id,
                project: String::new(),
                branch: String::new(),
                place: Place::None,
            })
        })
        .collect();
    Some(list)
}

/// The project a folder inside the workspace belongs to: the longest project
/// key its path relative to the root starts with.
pub fn project_within<'a>(rel: &Path, keys: &'a [String]) -> Option<&'a str> {
    keys.iter()
        .filter(|key| rel.starts_with(Path::new(key.as_str())))
        .max_by_key(|key| key.len())
        .map(String::as_str)
}

/// Every Claude session on this machine, sorted by name and placed against
/// the workspace at `root`. `keys` are its project keys and `slugs` maps a
/// lowercased GitHub `owner/name` to its project. `None` when `claude` could
/// not list them.
pub fn read(root: &Path, keys: &[String], slugs: &HashMap<String, String>) -> Option<Vec<Session>> {
    let mut command = Command::new("claude");
    command.args(["agents", "--json", "--all"]);
    let (_, stdout) = super::run(command, TIMEOUT)?;
    let mut list = parse_agents(&stdout)?;
    let mut placed: HashMap<String, (String, String, Place)> = HashMap::new();
    for session in &mut list {
        if session.cwd.is_empty() {
            continue;
        }
        let (project, branch, place) = placed
            .entry(session.cwd.clone())
            .or_insert_with(|| place(root, keys, slugs, Path::new(&session.cwd)))
            .clone();
        session.project = project;
        session.branch = branch;
        session.place = place;
    }
    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Some(list)
}

fn place(
    root: &Path,
    keys: &[String],
    slugs: &HashMap<String, String>,
    cwd: &Path,
) -> (String, String, Place) {
    let real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if real == root {
        return (String::new(), String::new(), Place::Root);
    }
    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if let Ok(rel) = real.strip_prefix(root) {
        let project = project_within(rel, keys).unwrap_or_default().to_string();
        return (project, branch, Place::In);
    }
    let project = git(cwd, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
        .and_then(|slug| slugs.get(&slug.to_lowercase()).cloned());
    match project {
        Some(project) => (project, branch, Place::Out),
        None => (String::new(), branch, Place::None),
    }
}

/// The board row a session belongs to: the branch checked out in the folder
/// it runs in, else the branch its name points at. A background agent started
/// on an issue may run from the workspace root, named `<project>-<issue>`:
/// the name still says which branch is its own.
pub fn item_of<'a>(session: &Session, board: &'a [Item]) -> Option<&'a Item> {
    if !session.project.is_empty() && !session.branch.is_empty() {
        if let Some(own) = board
            .iter()
            .find(|item| item.project == session.project && item.branch == session.branch)
        {
            return Some(own);
        }
    }
    if session.kind != "background" {
        return None;
    }
    static NAMED: OnceLock<Regex> = OnceLock::new();
    let named = NAMED
        .get_or_init(|| Regex::new(r"^(.+)-(\d+)$").expect("a valid pattern"))
        .captures(&session.name)?;
    let project = named.get(1)?.as_str();
    let issue: u64 = named.get(2)?.as_str().parse().ok()?;
    board.iter().find(|item| {
        item.project == project && super::github::issue_of_branch(&item.branch) == Some(issue)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::dash::scan::Section;

    fn item(project: &str, branch: &str) -> Item {
        Item {
            project: project.into(),
            branch: branch.into(),
            path: String::new(),
            is_default: false,
            dirty: 0,
            unpushed: 0,
            has_upstream: true,
            date: String::new(),
            pr: None,
            section: Section::Idle,
        }
    }

    #[test]
    fn agents_are_read_from_the_cli_list() {
        let out = r#"[
            {"id":"aaa11111","sessionId":"aaa11111-0000","name":"app-62","kind":"background","status":"busy","startedAt":1700000000000,"cwd":"/ws"},
            {"sessionId":"bbb22222-0000","kind":"interactive","status":"idle","state":"done","cwd":"/ws/app/main"},
            {"id":"ccc33333","sessionId":"ccc33333-0000","name":"asks","kind":"background","status":"permission","cwd":""},
            {"name":"no id"},
            7
        ]"#;
        let list = parse_agents(out).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id, "aaa11111");
        assert_eq!(list[0].started_at, 1_700_000_000_000);
        assert!(!list[0].needs_you);
        // No id or name: both fall back to the start of the session id.
        assert_eq!(list[1].id, "bbb22222");
        assert_eq!(list[1].name, "bbb22222");
        assert_eq!(list[1].status, "done");
        assert!(list[2].needs_you);
        assert!(parse_agents("{}").is_none());
        assert!(parse_agents("not json").is_none());
    }

    #[test]
    fn a_folder_belongs_to_the_longest_matching_project() {
        let keys = vec![
            "team".to_string(),
            "team/api".to_string(),
            "app".to_string(),
        ];
        assert_eq!(
            project_within(Path::new("team/api/main/src"), &keys),
            Some("team/api")
        );
        assert_eq!(
            project_within(Path::new("app/feat/1-x"), &keys),
            Some("app")
        );
        assert_eq!(project_within(Path::new("docs"), &keys), None);
        // A name that only starts the same is another folder.
        assert_eq!(project_within(Path::new("application"), &keys), None);
    }

    #[test]
    fn a_session_finds_its_board_row() {
        let board = vec![item("app", "feat/62-serve"), item("app", "main")];
        let mut session = parse_agents(
            r#"[{"id":"a","sessionId":"a-0","name":"app-62","kind":"background","status":"busy","cwd":"/ws"}]"#,
        )
        .unwrap()
        .remove(0);
        // Run from the workspace root: the name says which branch is its own.
        assert_eq!(
            item_of(&session, &board).map(Item::key).as_deref(),
            Some("app/feat/62-serve")
        );
        session.project = "app".into();
        session.branch = "main".into();
        assert_eq!(
            item_of(&session, &board).map(Item::key).as_deref(),
            Some("app/main")
        );
        session.kind = "interactive".into();
        session.branch = "gone".into();
        assert!(item_of(&session, &board).is_none());
    }
}
