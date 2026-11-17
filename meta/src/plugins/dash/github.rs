//! The GitHub half of the dashboard: open pull requests and issues, asked for
//! every project in one GraphQL request through the `gh` CLI, so the user's
//! own login applies.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use super::scan::{Ci, Item, PullRequest};

/// A project GitHub can be asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub project: String,
    /// `owner/name`.
    pub slug: String,
}

/// How many repositories one request asks about.
const REPOS_A_REQUEST: usize = 40;
/// A repository's issues come a hundred to a page: this many pages are read.
const ISSUE_PAGES: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(60);

const PRS_ASKED: &str = "pullRequests(states: OPEN, first: 100, orderBy: {field: UPDATED_AT, direction: DESC}) { nodes { number title headRefName isDraft reviewDecision updatedAt commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } } }";

fn issues_asked(after: &str) -> String {
    let after = if after.is_empty() {
        String::new()
    } else {
        format!(", after: {}", Value::from(after))
    };
    format!("issues(states: OPEN, first: 100{after}, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ number title updatedAt labels(first: 30) {{ nodes {{ name }} }} assignees(first: 10) {{ nodes {{ login }} }} }} }}")
}

/// One GraphQL query asking the same thing of several repositories, each under
/// an alias of its own (`r0`, `r1`, ...). `fields` is what is asked of the nth.
pub fn query_for(repos: &[Repo], fields: impl Fn(usize) -> String) -> String {
    let asked: Vec<String> = repos
        .iter()
        .enumerate()
        .map(|(n, repo)| {
            let (owner, name) = repo
                .slug
                .split_once('/')
                .unwrap_or((repo.slug.as_str(), ""));
            format!(
                "r{n}: repository(owner: {}, name: {}) {{ {} }}",
                Value::from(owner),
                Value::from(name),
                fields(n)
            )
        })
        .collect();
    format!("query {{\n{}\n}}", asked.join("\n"))
}

/// What GitHub said of each repository, in order: `None` for one it gave
/// nothing for (gone, or not readable), and all `None` when it gave no answer.
fn ask(repos: &[Repo], fields: impl Fn(usize) -> String) -> Vec<Option<Value>> {
    let mut command = Command::new("gh");
    command.args(["api", "graphql", "-f"]);
    command.arg(format!("query={}", query_for(repos, fields)));
    // gh prints what GitHub answered even when one repository in it failed.
    let answer: Option<Value> =
        super::run(command, TIMEOUT).and_then(|(_, stdout)| serde_json::from_str(&stdout).ok());
    repos
        .iter()
        .enumerate()
        .map(|(n, _)| {
            answer
                .as_ref()
                .and_then(|answer| answer.get("data")?.get(format!("r{n}")))
                .filter(|one| one.is_object())
                .cloned()
        })
        .collect()
}

fn nodes(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(|value| value.get("nodes"))
        .and_then(Value::as_array)
        .map(|nodes| nodes.iter().filter(|node| node.is_object()).collect())
        .unwrap_or_default()
}

/// A string field as one line: no tabs or line breaks.
fn one_line(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(|text| text.replace(['\t', '\n', '\r'], " "))
        .unwrap_or_default()
}

/// A branch's open pull request, with the day it was last updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPr {
    pub branch: String,
    pub updated: String,
    pub pr: PullRequest,
}

/// A repository's open pull requests, as GitHub answered them.
pub fn prs_of(repository: &Value) -> Vec<BranchPr> {
    nodes(repository.get("pullRequests"))
        .into_iter()
        .map(|node| {
            let state = nodes(node.get("commits"))
                .first()
                .and_then(|commit| commit.get("commit")?.get("statusCheckRollup")?.get("state"))
                .and_then(Value::as_str);
            let ci = match state {
                Some("SUCCESS") => Ci::Pass,
                Some("FAILURE") | Some("ERROR") => Ci::Fail,
                Some("PENDING") | Some("EXPECTED") => Ci::Pending,
                _ => Ci::None,
            };
            BranchPr {
                branch: one_line(node.get("headRefName")),
                updated: one_line(node.get("updatedAt")).chars().take(10).collect(),
                pr: PullRequest {
                    number: node.get("number").and_then(Value::as_u64).unwrap_or(0),
                    title: one_line(node.get("title")),
                    is_draft: node
                        .get("isDraft")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    review: one_line(node.get("reviewDecision")),
                    ci,
                },
            }
        })
        .collect()
}

/// An open issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub project: String,
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Day of the last update (`YYYY-MM-DD`).
    pub updated: String,
    pub updated_at: String,
    /// Rank from a `priority:` label, 0 the most urgent; `None` without one.
    pub priority: Option<u8>,
    /// Areas from `area:` labels, lowercased.
    pub areas: Vec<String>,
    /// Whether a branch on the board already works on it.
    pub in_flight: bool,
}

/// A priority label as a rank, 0 the most urgent: the p0-p4 scheme and the
/// named one (high, medium, low) are the same scale.
pub fn priority_of(labels: &[String]) -> Option<u8> {
    labels
        .iter()
        .filter_map(|label| {
            let lower = label.to_lowercase();
            let named = lower
                .strip_prefix("priority:")
                .or_else(|| lower.strip_prefix("priority/"))?
                .trim()
                .to_string();
            match named.as_str() {
                "p0" | "critical" | "urgent" | "blocker" => Some(0),
                "p1" | "high" => Some(1),
                "p2" | "medium" | "med" | "normal" => Some(2),
                "p3" | "low" => Some(3),
                "p4" | "lowest" | "trivial" => Some(4),
                _ => None,
            }
        })
        .min()
}

/// The areas an issue is labelled with: `area:mcp` and `area/agent` give mcp, agent.
pub fn areas_of(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| {
            let lower = label.to_lowercase();
            let area = lower
                .strip_prefix("area:")
                .or_else(|| lower.strip_prefix("area/"))?;
            Some(area.trim().to_string())
        })
        .collect()
}

/// One page of a repository's open issues, as GitHub answered it.
pub fn issues_of(project: &str, repository: &Value) -> Vec<Issue> {
    nodes(repository.get("issues"))
        .into_iter()
        .map(|node| {
            let names = |field: &str, key: &str| -> Vec<String> {
                nodes(node.get(field))
                    .into_iter()
                    .map(|one| one_line(one.get(key)))
                    .filter(|name| !name.is_empty())
                    .collect()
            };
            let labels = names("labels", "name");
            let updated_at = one_line(node.get("updatedAt"));
            Issue {
                project: project.to_string(),
                number: node.get("number").and_then(Value::as_u64).unwrap_or(0),
                title: one_line(node.get("title")),
                assignees: names("assignees", "login"),
                updated: updated_at.chars().take(10).collect(),
                updated_at,
                priority: priority_of(&labels),
                areas: areas_of(&labels),
                labels,
                in_flight: false,
            }
        })
        .collect()
}

/// The issue number a branch name leads with: `feat/62-serve`,
/// `lane/185-pointer` and `issue-172-cap` give 62, 185 and 172.
pub fn issue_of_branch(branch: &str) -> Option<u64> {
    static LEADING: OnceLock<Regex> = OnceLock::new();
    LEADING
        .get_or_init(|| Regex::new(r"(?:^|/)(?:issue-)?(\d+)-").expect("a valid pattern"))
        .captures(branch)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

/// Mark the issues a branch on the board already works on.
pub fn mark_in_flight(issues: &mut [Issue], board: &[Item]) {
    let working: HashSet<(&str, u64)> = board
        .iter()
        .filter_map(|item| Some((item.project.as_str(), issue_of_branch(&item.branch)?)))
        .collect();
    for issue in issues {
        issue.in_flight = working.contains(&(issue.project.as_str(), issue.number));
    }
}

/// Every project's open pull requests by project, and the projects GitHub
/// gave no answer for.
pub fn read_prs(repos: &[Repo]) -> (BTreeMap<String, Vec<BranchPr>>, Vec<String>) {
    let mut found = BTreeMap::new();
    let mut failed = Vec::new();
    for chunk in repos.chunks(REPOS_A_REQUEST) {
        let answers = ask(chunk, |_| PRS_ASKED.to_string());
        for (repo, answer) in chunk.iter().zip(answers) {
            match answer {
                Some(repository) => {
                    found.insert(repo.project.clone(), prs_of(&repository));
                }
                None => failed.push(repo.project.clone()),
            }
        }
    }
    (found, failed)
}

/// Every project's open issues, newest first, and the projects GitHub gave no
/// answer for. Projects with more than a page are asked again, up to
/// [`ISSUE_PAGES`] pages each.
pub fn read_issues(repos: &[Repo]) -> (Vec<Issue>, Vec<String>) {
    struct Asking<'a> {
        repo: &'a Repo,
        after: String,
    }
    let mut by_key: BTreeMap<(String, u64), Issue> = BTreeMap::new();
    let mut failed = Vec::new();
    let mut asks: Vec<Asking> = repos
        .iter()
        .map(|repo| Asking {
            repo,
            after: String::new(),
        })
        .collect();
    for page in 1..=ISSUE_PAGES {
        if asks.is_empty() {
            break;
        }
        let mut more = Vec::new();
        for chunk in asks.chunks(REPOS_A_REQUEST) {
            let chunk_repos: Vec<Repo> = chunk.iter().map(|ask| ask.repo.clone()).collect();
            let answers = ask(&chunk_repos, |n| issues_asked(&chunk[n].after));
            for (asking, answer) in chunk.iter().zip(answers) {
                let Some(repository) = answer else {
                    failed.push(asking.repo.project.clone());
                    continue;
                };
                // An issue updated between two pages can be on both: keyed, it is kept once.
                for issue in issues_of(&asking.repo.project, &repository) {
                    by_key.insert((issue.project.clone(), issue.number), issue);
                }
                let info = repository
                    .get("issues")
                    .and_then(|issues| issues.get("pageInfo"));
                let has_next = info
                    .and_then(|info| info.get("hasNextPage"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let cursor = info
                    .and_then(|info| info.get("endCursor"))
                    .and_then(Value::as_str);
                if let (true, Some(cursor), true) = (has_next, cursor, page < ISSUE_PAGES) {
                    more.push(Asking {
                        repo: asking.repo,
                        after: cursor.to_string(),
                    });
                }
            }
        }
        asks = more;
    }
    let mut issues: Vec<Issue> = by_key.into_values().collect();
    issues.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(a.project.cmp(&b.project))
            .then(b.number.cmp(&a.number))
    });
    failed.sort();
    failed.dedup();
    (issues, failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_query_asks_every_repository_under_its_own_alias() {
        let repos = [
            Repo {
                project: "a".into(),
                slug: "org/a".into(),
            },
            Repo {
                project: "b".into(),
                slug: "org/b\"x".into(),
            },
        ];
        let query = query_for(&repos, |n| format!("f{n}"));
        assert!(query.contains(r#"r0: repository(owner: "org", name: "a") { f0 }"#));
        // A quote in a name cannot end the string it is written in.
        assert!(query.contains(r#"r1: repository(owner: "org", name: "b\"x") { f1 }"#));
        assert!(issues_asked("abc").contains(r#", after: "abc""#));
        assert!(!issues_asked("").contains("after"));
    }

    #[test]
    fn pull_requests_are_read_with_their_checks() {
        let repository = json!({ "pullRequests": { "nodes": [
            { "number": 7, "title": "feat: a\tthing", "headRefName": "feat/7-thing", "isDraft": true,
              "reviewDecision": "CHANGES_REQUESTED", "updatedAt": "2026-11-12T10:00:00Z",
              "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "FAILURE" } } } ] } },
            { "number": 8, "title": "no checks", "headRefName": "fix/8", "isDraft": false,
              "reviewDecision": null, "updatedAt": "2026-11-10T10:00:00Z",
              "commits": { "nodes": [ { "commit": { "statusCheckRollup": null } } ] } }
        ] } });
        let prs = prs_of(&repository);
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].branch, "feat/7-thing");
        assert_eq!(prs[0].updated, "2026-11-12");
        assert_eq!(prs[0].pr.title, "feat: a thing");
        assert!(prs[0].pr.is_draft);
        assert_eq!(prs[0].pr.review, "CHANGES_REQUESTED");
        assert_eq!(prs[0].pr.ci, Ci::Fail);
        assert_eq!(prs[1].pr.ci, Ci::None);
        assert_eq!(prs[1].pr.review, "");
        assert!(prs_of(&json!({})).is_empty());
    }

    #[test]
    fn issues_carry_priority_and_areas() {
        let repository = json!({ "issues": { "nodes": [
            { "number": 62, "title": "Serve it", "updatedAt": "2026-11-12T10:00:00Z",
              "labels": { "nodes": [ { "name": "priority:P1" }, { "name": "area/MCP" }, { "name": "bug" } ] },
              "assignees": { "nodes": [ { "login": "someone" } ] } }
        ] } });
        let issues = issues_of("app", &repository);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].project, "app");
        assert_eq!(issues[0].updated, "2026-11-12");
        assert_eq!(issues[0].priority, Some(1));
        assert_eq!(issues[0].areas, vec!["mcp"]);
        assert_eq!(issues[0].assignees, vec!["someone"]);
        assert_eq!(issues[0].labels.len(), 3);
    }

    #[test]
    fn the_most_urgent_priority_label_wins() {
        assert_eq!(
            priority_of(&labels(&["priority: low", "priority/urgent"])),
            Some(0)
        );
        assert_eq!(priority_of(&labels(&["priority:medium"])), Some(2));
        assert_eq!(priority_of(&labels(&["priority:someday", "P1"])), None);
    }

    #[test]
    fn a_branch_names_the_issue_it_works_on() {
        assert_eq!(issue_of_branch("feat/62-serve"), Some(62));
        assert_eq!(issue_of_branch("issue-172-cap"), Some(172));
        assert_eq!(issue_of_branch("lane/185-pointer"), Some(185));
        assert_eq!(issue_of_branch("main"), None);
        assert_eq!(issue_of_branch("release-2"), None);
    }
}
