//! Starting work on an issue: a branch and worktree for it, then a Claude
//! Code session there with the issue as its prompt. In a host that opens
//! panes the session runs beside the dashboard; elsewhere it runs in the
//! background and is attached to by hand.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use super::host::{self, Host, Runner};
use super::scan::{git, github_slug, parse_worktrees};

/// The issue to start on, as the dashboard read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub project: String,
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
}

impl Wanted {
    /// The session's name, which also ties it back to its branch on the board.
    pub fn name(&self) -> String {
        format!("{}-{}", self.project, self.number)
    }
}

fn title_kind() -> &'static Regex {
    static KIND: OnceLock<Regex> = OnceLock::new();
    KIND.get_or_init(|| {
        Regex::new(
            r"(?i)^(feat|fix|docs|chore|refactor|test|perf|ci|build|style)(\([^)]*\))?!?:\s*",
        )
        .expect("a valid pattern")
    })
}

/// A label that names a kind of work, as the branch prefix it stands for. A
/// label of another sort (area:docs, priority:p1) says nothing about the kind.
fn kind_of_label(label: &str) -> Option<(usize, &'static str)> {
    let lower = label.to_lowercase();
    let name = ["type:", "type/", "kind:", "kind/"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or(&lower)
        .trim();
    // In this order: an issue labelled both bug and enhancement is a fix.
    let kinds: [(&[&str], &str); 8] = [
        (&["bug", "fix", "defect", "regression"], "fix"),
        (&["doc", "docs", "documentation"], "docs"),
        (&["chore", "maintenance", "dependencies", "deps"], "chore"),
        (
            &["refactor", "refactoring", "cleanup", "tech-debt"],
            "refactor",
        ),
        (&["test", "tests", "testing"], "test"),
        (&["perf", "performance"], "perf"),
        (&["ci", "build"], "ci"),
        (&["enhancement", "feature", "feat"], "feat"),
    ];
    kinds
        .iter()
        .enumerate()
        .find(|(_, (names, _))| names.contains(&name))
        .map(|(rank, (_, kind))| (rank, *kind))
}

/// `fix/435-claw-service-command`: the kind from the labels (a bug before an
/// enhancement), else from a conventional title, else feat; then the issue
/// number and the title's first words, to forty characters.
pub fn branch_for(issue: &Wanted) -> String {
    let from_label = issue
        .labels
        .iter()
        .filter_map(|label| kind_of_label(label))
        .min()
        .map(|(_, kind)| kind.to_string());
    let from_title = title_kind()
        .captures(&issue.title)
        .and_then(|found| found.get(1))
        .map(|kind| kind.as_str().to_lowercase());
    let kind = from_label
        .or(from_title)
        .unwrap_or_else(|| "feat".to_string());
    let plain = title_kind().replace(&issue.title, "").to_lowercase();
    let mut slug = String::new();
    for word in plain
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        if slug.is_empty() {
            slug = word.chars().take(40).collect();
        } else if slug.len() + 1 + word.len() > 40 {
            break;
        } else {
            slug = format!("{slug}-{word}");
        }
    }
    if slug.is_empty() {
        slug = "work".to_string();
    }
    format!("{kind}/{}-{slug}", issue.number)
}

/// Create `branch` and a worktree for it in `project`, from the remote's
/// default branch, by running `meta worktree add` (`meta` is the binary to
/// run) so the workspace's layout and hooks apply. Gives the worktree's path.
pub fn make_worktree(
    meta: &Path,
    root: &Path,
    project: &str,
    branch: &str,
) -> Result<PathBuf, String> {
    let dir = root.join(project);
    if !dir.join(".git").exists() {
        return Err(format!("no project named {project}"));
    }
    let mut fetch = Command::new("git");
    fetch.arg("-C").arg(&dir).args(["fetch", "-q", "origin"]);
    // Offline, the branch starts from what was last fetched.
    let _ = super::run(fetch, Duration::from_secs(60));
    let has = |name: &str| git(&dir, &["show-ref", "--verify", "-q", name]).is_some();
    if has(&format!("refs/heads/{branch}")) || has(&format!("refs/remotes/origin/{branch}")) {
        return Err(format!("branch {branch} already exists in {project}"));
    }
    let base = git(
        &dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .into_iter()
    .chain(["origin/main".to_string(), "HEAD".to_string()])
    .find(|base| {
        git(
            &dir,
            &["rev-parse", "-q", "--verify", &format!("{base}^{{commit}}")],
        )
        .is_some()
    })
    .ok_or_else(|| format!("{project} has no commit to branch from"))?;

    let mut add = Command::new(meta);
    add.current_dir(root)
        .args(["--non-interactive", "defaults", "worktree", "add", branch])
        .args(["--project", project, "--from", &base]);
    let said = super::run(add, Duration::from_secs(120))
        .map(|(_, stdout)| stdout)
        .unwrap_or_default();
    let path = git(&dir, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default()
        .into_iter()
        .find(|(path, checked_out)| checked_out == branch && path.is_dir())
        .map(|(path, _)| path)
        .ok_or_else(|| {
            let last = said
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("meta said nothing");
            format!("the worktree was not created: {}", last.trim())
        })?;
    // meta leaves the new branch tracking the base; a push must never aim there.
    let _ = git(&path, &["branch", "--unset-upstream"]);
    Ok(path)
}

/// What the session is told. `in_worktree` is whether it starts inside the
/// worktree (a pane) or in the workspace root (a background session, which
/// must be sent to its worktree and kept out of the other projects).
pub fn prompt_for(
    issue: &Wanted,
    slug: &str,
    branch: &str,
    path: &Path,
    body: &str,
    in_worktree: bool,
) -> String {
    let of = if slug.is_empty() {
        issue.project.as_str()
    } else {
        slug
    };
    let place = if in_worktree {
        format!(
            "You are in its worktree, {} (branch {branch}). Read and edit files only inside it.",
            path.display()
        )
    } else {
        format!(
            "Your worktree is {} (branch {branch}). Change into it first. Read and edit files only inside it: this session starts in the workspace root, which holds many other projects, and none of them is yours to touch.",
            path.display()
        )
    };
    let body: String = body.trim().chars().take(8000).collect();
    [
        format!("Work on GitHub issue #{} of {of}: {}", issue.number, issue.title),
        String::new(),
        place,
        String::new(),
        format!(
            "When the work is done, commit it in that worktree with a message that ends \"(refs #{})\". Do not push and do not open a pull request.",
            issue.number
        ),
        String::new(),
        "The issue:".to_string(),
        if body.is_empty() { "(the issue has no description)".to_string() } else { body },
    ]
    .join("\n")
}

/// What starting came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub name: String,
    /// The pane the session runs in, where the host opened one.
    pub pane: Option<u64>,
    /// One line for the person.
    pub said: String,
}

/// Open a pane in the worktree running Claude on the prompt. The control
/// socket cannot carry line breaks, so the prompt goes by a file only this
/// user can read, which the pane's shell reads and removes before it starts
/// Claude. `scratch` is the folder the file is written in.
pub fn launch_in_pane(
    run: Runner,
    scratch: &Path,
    name: &str,
    path: &Path,
    prompt: &str,
    model: Option<&str>,
) -> Result<u64, String> {
    std::fs::create_dir_all(scratch)
        .map_err(|e| format!("could not make {}: {e}", scratch.display()))?;
    let file = scratch.join(format!("{name}-{}.prompt", std::process::id()));
    write_private(&file, prompt).map_err(|e| format!("could not write the prompt: {e}"))?;
    // A model is named only when one was chosen: without, Claude's default.
    let script = r#"p=$(cat "$2"); rm -f "$2"; exec claude --name "$1" ${3:+--model "$3"} "$p""#;
    let argv = [
        "sh",
        "-c",
        script,
        "sh",
        name,
        &file.to_string_lossy(),
        model.unwrap_or(""),
    ]
    .map(String::from);
    host::open_pane(run, path, name, &argv).inspect_err(|_| {
        let _ = std::fs::remove_file(&file);
    })
}

fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)?.write_all(text.as_bytes())
}

/// The short id in what `claude --bg` printed (`claude attach <id>`).
pub fn background_id(stdout: &str) -> Option<String> {
    static ID: OnceLock<Regex> = OnceLock::new();
    ID.get_or_init(|| Regex::new(r"claude (?:attach|stop) ([0-9a-f]+)").expect("a valid pattern"))
        .captures(stdout)
        .and_then(|found| found.get(1))
        .map(|id| id.as_str().to_string())
}

/// Create the issue's worktree and start a session on it: in a pane where the
/// host opens them, in the background otherwise. Slow (a fetch, a worktree,
/// a question to GitHub): run it off the interface's thread.
pub fn start(
    meta: &Path,
    root: &Path,
    host: Host,
    issue: &Wanted,
    model: Option<&str>,
) -> Result<Started, String> {
    let branch = branch_for(issue);
    let name = issue.name();
    let path = make_worktree(meta, root, &issue.project, &branch)?;
    let slug = git(&root.join(&issue.project), &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
        .unwrap_or_default();
    let body = if slug.is_empty() {
        String::new()
    } else {
        let mut view = Command::new("gh");
        view.args(["issue", "view", &issue.number.to_string(), "-R", &slug])
            .args(["--json", "body", "-q", ".body"]);
        super::run(view, Duration::from_secs(30))
            .filter(|(ok, _)| *ok)
            .map(|(_, body)| body)
            .unwrap_or_default()
    };
    let made = format!("the worktree is at {}", path.display());
    match host {
        Host::GlxyTerm => {
            let prompt = prompt_for(issue, &slug, &branch, &path, &body, true);
            let scratch = std::env::temp_dir().join("meta-dash");
            let pane = launch_in_pane(&host::glxy_term, &scratch, &name, &path, &prompt, model)
                .map_err(|why| format!("{made}, but no pane opened: {why}"))?;
            Ok(Started {
                said: format!("started {name} beside this pane on {branch}"),
                name,
                pane: Some(pane),
            })
        }
        Host::None => {
            let prompt = prompt_for(issue, &slug, &branch, &path, &body, false);
            let mut claude = Command::new("claude");
            claude.current_dir(root).args([
                "--bg",
                "--name",
                &name,
                "--permission-mode",
                "acceptEdits",
            ]);
            if let Some(model) = model {
                claude.args(["--model", model]);
            }
            claude.arg(prompt);
            let said = super::run(claude, Duration::from_secs(60))
                .map(|(_, stdout)| stdout)
                .unwrap_or_default();
            let id = background_id(&said).ok_or_else(|| {
                let last = said
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("claude could not be run");
                format!("{made}, but no session started: {}", last.trim())
            })?;
            Ok(Started {
                said: format!("started {name} in the background on {branch} · claude attach {id}"),
                name,
                pane: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn issue(number: u64, title: &str, labels: &[&str]) -> Wanted {
        Wanted {
            project: "claw".into(),
            number,
            title: title.into(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_branch_is_named_for_the_kind_number_and_title() {
        assert_eq!(
            branch_for(&issue(435, "Claw service command", &["bug"])),
            "fix/435-claw-service-command"
        );
        // A bug before an enhancement, whatever order the labels come in.
        assert_eq!(
            branch_for(&issue(1, "X", &["enhancement", "type: Bug"])),
            "fix/1-x"
        );
        // No kind in the labels: the conventional title says, and is not repeated.
        assert_eq!(
            branch_for(&issue(
                62,
                "docs(readme): Explain the tiers",
                &["area:docs", "priority:p1"]
            )),
            "docs/62-explain-the-tiers"
        );
        assert_eq!(branch_for(&issue(7, "Serve it", &[])), "feat/7-serve-it");
        assert_eq!(branch_for(&issue(9, "!!!", &[])), "feat/9-work");
        // Whole words, to forty characters.
        assert_eq!(
            branch_for(&issue(
                254,
                "Desktop provider test pane: chat with a provider directly and broadcast",
                &[]
            )),
            "feat/254-desktop-provider-test-pane-chat-with-a"
        );
    }

    #[test]
    fn the_prompt_says_where_to_work_and_how_to_finish() {
        let wanted = issue(62, "Serve it", &[]);
        let path = Path::new("/ws/claw/feat/62-serve-it");
        let here = prompt_for(
            &wanted,
            "org/claw",
            "feat/62-serve-it",
            path,
            "  Do the thing.\n",
            true,
        );
        assert!(here.starts_with("Work on GitHub issue #62 of org/claw: Serve it\n\nYou are in its worktree, /ws/claw/feat/62-serve-it (branch feat/62-serve-it)."));
        assert!(here.contains("ends \"(refs #62)\". Do not push"));
        assert!(here.ends_with("The issue:\nDo the thing."));
        let away = prompt_for(&wanted, "", "feat/62-serve-it", path, "", false);
        assert!(away.starts_with("Work on GitHub issue #62 of claw: Serve it"));
        assert!(away.contains("Change into it first."));
        assert!(away.ends_with("(the issue has no description)"));
        let long = prompt_for(&wanted, "org/claw", "b", path, &"x".repeat(9000), true);
        assert!(long.ends_with(&"x".repeat(8000)) && !long.ends_with(&"x".repeat(8001)));
    }

    #[test]
    fn a_pane_gets_its_prompt_by_a_private_file() {
        let scratch = tempfile::TempDir::new().unwrap();
        let asked = RefCell::new(Vec::new());
        let run = |args: &[String]| {
            asked.borrow_mut().push(args.to_vec());
            Some("pane 9\n".to_string())
        };
        let prompt = "Work on it.\n\nTwo paragraphs, a\ttab.";
        let pane = launch_in_pane(
            &run,
            scratch.path(),
            "claw-62",
            Path::new("/ws/claw/feat/62-x"),
            prompt,
            None,
        );
        assert_eq!(pane, Ok(9));
        let args = asked.borrow()[0].clone();
        assert_eq!(
            &args[..8],
            [
                "ctl",
                "open-pane",
                "--cwd",
                "/ws/claw/feat/62-x",
                "--title",
                "claw-62",
                "--right",
                "--"
            ]
        );
        assert_eq!(&args[8..10], ["sh", "-c"]);
        assert_eq!(args[12], "claw-62");
        // Nothing sent holds a line break; the prompt is whole in the file.
        assert!(args.iter().all(|arg| !arg.contains(['\n', '\t'])));
        assert_eq!(std::fs::read_to_string(&args[13]).unwrap(), prompt);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&args[13]).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // The pane's shell hands Claude the prompt and removes the file.
        let script = args[10].replace("exec claude --name", "printf '%s|%s'");
        let out = Command::new("sh")
            .args(["-c", &script, "sh", "claw-62", &args[13]])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("claw-62|{prompt}")
        );
        assert!(!Path::new(&args[13]).exists());

        // A model, when one was chosen, is named to Claude; none, and it is not.
        let pane = launch_in_pane(
            &run,
            scratch.path(),
            "claw-64",
            Path::new("/ws"),
            "hi",
            Some("claude-opus-5-5"),
        );
        assert_eq!(pane, Ok(9));
        let args = asked.borrow()[1].clone();
        assert_eq!(args[14], "claude-opus-5-5");
        let script = args[10].replace("exec claude", "printf '%s,'");
        let out = Command::new("sh")
            .args(["-c", &script])
            .args(&args[11..])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "--name,claw-64,--model,claude-opus-5-5,hi,"
        );
        let out = Command::new("sh")
            .args(["-c", &script, "sh", "claw-64", "/nowhere", ""])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "--name,claw-64,,");

        // A refusal leaves no prompt behind.
        let refused = |_: &[String]| Some("error: no window\n".to_string());
        assert!(launch_in_pane(
            &refused,
            scratch.path(),
            "claw-63",
            Path::new("/ws"),
            prompt,
            None
        )
        .is_err());
        assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_background_id_is_read_from_what_claude_printed() {
        assert_eq!(
            background_id(
                "Started in the background.\n  claude attach 09f0f7db\n  claude stop 09f0f7db\n"
            )
            .as_deref(),
            Some("09f0f7db")
        );
        assert_eq!(background_id("error: not logged in"), None);
    }
}
