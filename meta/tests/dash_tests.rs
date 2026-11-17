// End-to-end tests for `meta dash`: the real binary against a temp workspace
// of local git repos. GitHub and the session list are switched off, so no
// network and no other CLI is needed.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

const META_BIN: &str = env!("CARGO_BIN_EXE_meta");

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run git in `dir`, committing as of `days_ago` days back.
fn run_git(dir: &Path, days_ago: u64, args: &[&str]) {
    let when = format!(
        "{} +0000",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - days_ago * 86_400
    );
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test"])
        .args(["-c", "user.email=test@example.com"])
        .args(args)
        .env("GIT_AUTHOR_DATE", &when)
        .env("GIT_COMMITTER_DATE", &when)
        .status()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &Path, days_ago: u64) {
    fs::create_dir_all(dir).unwrap();
    run_git(dir, days_ago, &["init", "-q", "-b", "main"]);
    run_git(
        dir,
        days_ago,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
}

fn dash(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(META_BIN)
        .current_dir(dir)
        .arg("dash")
        .args(args)
        .args(["--no-github", "--no-sessions"])
        // Cost is read from this folder alone, never the real transcripts.
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
        .stdin(Stdio::null())
        .output()
        .expect("meta runs");
    assert!(
        out.status.success(),
        "meta dash {args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A workspace of three projects: `app` (active, a branch in flight), `old`
/// (active, last commit 200 days ago) and `lib` (no tier).
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("app"), 0);
    run_git(
        &root.join("app"),
        0,
        &["checkout", "-q", "-b", "feat/7-thing"],
    );
    run_git(
        &root.join("app"),
        0,
        &["commit", "-q", "--allow-empty", "-m", "work"],
    );
    run_git(&root.join("app"), 0, &["checkout", "-q", "main"]);
    init_repo(&root.join("old"), 200);
    init_repo(&root.join("lib"), 0);
    fs::write(
        root.join(".meta"),
        r#"{
  "projects": {
    "app": { "url": "git@github.com:example/app.git", "labels": ["active", "rust"] },
    "old": { "url": "git@github.com:example/old.git", "labels": ["active"] },
    "lib": "git@gitlab.com:example/lib.git"
  }
}
"#,
    )
    .unwrap();
    tmp
}

fn project<'a>(snapshot: &'a Value, name: &str) -> &'a Value {
    snapshot["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no project {name} in {snapshot}"))
}

#[test]
fn json_reports_tiers_work_and_the_board() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let snapshot: Value = serde_json::from_str(&dash(tmp.path(), &["--json"])).unwrap();
    assert_eq!(snapshot["schema"], 1);
    assert_eq!(snapshot["has_tiers"], true);
    assert_eq!(snapshot["projects"].as_array().unwrap().len(), 3);
    assert_eq!(snapshot["github"]["asked"], false);

    // A branch committed to lately is work in flight: app stays active.
    let app = project(&snapshot, "app");
    assert_eq!(app["state"], "ok");
    assert_eq!(app["tier"], "active");
    assert_eq!(app["tier_due"], "active");
    assert_eq!(app["branch"], "main");
    assert_eq!(app["quiet_days"], 0);
    assert!(app["work"].as_u64().unwrap() >= 1);
    // The slug comes from the config where the clone has no origin.
    assert_eq!(app["slug"], "example/app");

    // 200 quiet days and nothing in flight: due dormant, reported, not applied.
    let old = project(&snapshot, "old");
    assert_eq!(old["tier"], "active");
    assert_eq!(old["tier_due"], "dormant");
    assert_eq!(old["work"], 0);
    assert!(old["quiet_days"].as_u64().unwrap() >= 199);
    let moves = snapshot["retier"].as_array().unwrap();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0]["project"], "old");
    assert_eq!(moves[0]["from"], "active");
    assert_eq!(moves[0]["to"], "dormant");
    assert_eq!(moves[0]["applied"], false);

    // A project with no tier label has none and is due none.
    let lib = project(&snapshot, "lib");
    assert!(lib["tier"].is_null() && lib["tier_due"].is_null());
    assert!(lib["slug"].is_null());

    let branch = snapshot["board"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["project"] == "app" && i["branch"] == "feat/7-thing")
        .expect("the unpushed branch is on the board");
    assert!(branch["unpushed"].as_u64().unwrap() >= 1);
    assert_eq!(branch["has_upstream"], false);
    assert_eq!(branch["path"], "");
    assert_eq!(branch["section"], "local");
    assert!(branch["pr"].is_null());
}

#[test]
fn retier_rewrites_only_the_tier_label() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let snapshot: Value = serde_json::from_str(&dash(tmp.path(), &["--json", "--retier"])).unwrap();
    assert_eq!(snapshot["retier"][0]["applied"], true);
    assert_eq!(project(&snapshot, "old")["tier"], "dormant");

    let config: Value =
        serde_json::from_str(&fs::read_to_string(tmp.path().join(".meta")).unwrap()).unwrap();
    assert_eq!(
        config["projects"]["old"]["labels"],
        serde_json::json!(["dormant"])
    );
    // Other labels and other projects are left as they were.
    assert_eq!(
        config["projects"]["app"]["labels"],
        serde_json::json!(["active", "rust"])
    );
    assert_eq!(config["projects"]["lib"], "git@gitlab.com:example/lib.git");

    // Nothing is left to move on the next read.
    let again: Value = serde_json::from_str(&dash(tmp.path(), &["--json"])).unwrap();
    assert!(again["retier"].as_array().unwrap().is_empty());
}

#[test]
fn summary_groups_projects_by_tier() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let text = dash(tmp.path(), &[]);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{text}"))
    };
    assert!(at("Active (2)") < at("No tier (1)"));
    assert!(text.contains("Tier due  old active to dormant"), "{text}");
    assert!(text.contains("apply with --retier"), "{text}");
}

#[test]
fn cost_is_summed_from_transcripts_by_project() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let root = tmp.path().canonicalize().unwrap();
    let reply = |id: &str, cwd: &Path, model: &str| {
        serde_json::json!({
            "type": "assistant", "requestId": format!("req-{id}"), "sessionId": "aaa11111-0000",
            "cwd": cwd, "timestamp": "2026-11-12T10:00:00Z",
            "message": { "id": format!("msg-{id}"), "model": model,
                         "usage": { "input_tokens": 1_000_000, "output_tokens": 0 } }
        })
        .to_string()
    };
    let transcripts = tmp
        .path()
        .join("claude")
        .join("projects")
        .join("some-session");
    fs::create_dir_all(&transcripts).unwrap();
    fs::write(
        transcripts.join("aaa11111.jsonl"),
        [
            reply("1", &root.join("app"), "claude-sonnet-5-5"),
            // The same reply written twice counts once.
            reply("1", &root.join("app"), "claude-sonnet-5-5"),
            reply("2", &root, "claude-sonnet-5-5"),
            reply(
                "3",
                Path::new("/somewhere/else/scratch"),
                "claude-mystery-9",
            ),
            // A line still being written is left for the next read.
            "{\"type\":\"assistant\",\"message\":{\"usage\"".to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let snapshot: Value = serde_json::from_str(&dash(tmp.path(), &["--json"])).unwrap();
    let rows = snapshot["cost"]["rows"].as_array().unwrap();
    let usd = |bucket: &str| {
        rows.iter()
            .find(|row| row["bucket"] == bucket)
            .unwrap_or_else(|| panic!("no {bucket} in {rows:?}"))["usd"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(rows.len(), 3);
    assert!((usd("app") - 2.0).abs() < 1e-9);
    assert!((usd("@root") - 2.0).abs() < 1e-9);
    assert_eq!(usd("@out:scratch"), 0.0);
    assert_eq!(
        snapshot["cost"]["unpriced"],
        serde_json::json!(["claude-mystery-9"])
    );
    assert_eq!(snapshot["cost"]["sessions"][0]["tokens"], 3_000_000);

    // With no transcript folder, or when told not to, there is no cost at all.
    let skipped: Value = serde_json::from_str(&dash(tmp.path(), &["--json", "--no-cost"])).unwrap();
    assert!(skipped["cost"].is_null());
}

#[test]
fn starting_work_makes_a_branch_and_worktree_through_meta() {
    use metarepo::plugins::dash::start::make_worktree;
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let root = tmp.path().canonicalize().unwrap();
    let meta = Path::new(META_BIN);

    let path = make_worktree(meta, &root, "app", "fix/8-new-thing").expect("a worktree is made");
    assert!(path.is_dir(), "{} is not a folder", path.display());
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };
    assert_eq!(
        git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        (true, "fix/8-new-thing".to_string())
    );
    // The new branch tracks nothing, so a push cannot aim at the base.
    assert!(!git(&["rev-parse", "-q", "--verify", "@{u}"]).0);

    // The board then shows it, and the issue it is named for counts as in flight.
    let snapshot: Value = serde_json::from_str(&dash(&root, &["--json"])).unwrap();
    assert!(snapshot["board"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["project"] == "app" && i["branch"] == "fix/8-new-thing" && i["path"] != ""));

    let again = make_worktree(meta, &root, "app", "fix/8-new-thing").unwrap_err();
    assert_eq!(again, "branch fix/8-new-thing already exists in app");
    assert_eq!(
        make_worktree(meta, &root, "nope", "fix/1-x").unwrap_err(),
        "no project named nope"
    );
}

/// The workspace as a git repository of its own, with a README whose project
/// table carries tiers, all committed.
fn committed_workspace() -> TempDir {
    let tmp = workspace();
    let root = tmp.path();
    fs::write(root.join(".gitignore"), "app/\nold/\nlib/\nclaude/\n").unwrap();
    fs::write(
        root.join("README.md"),
        "# Workspace\n\n| Project | Tier | Description |\n| --- | --- | --- |\n| [app](https://github.com/example/app) | active | The app |\n| [old](https://github.com/example/old) | active | Not touched lately |\n| lib | | No tier |\n",
    )
    .unwrap();
    run_git(root, 0, &["init", "-q", "-b", "main"]);
    run_git(root, 0, &["add", "-A"]);
    run_git(root, 0, &["commit", "-q", "-m", "init"]);
    tmp
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `meta dash` with the committer set, as the commit it makes needs one.
fn dash_committing(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(META_BIN)
        .current_dir(dir)
        .arg("dash")
        .args(args)
        .args(["--no-github", "--no-sessions", "--no-cost"])
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .stdin(Stdio::null())
        .output()
        .expect("meta runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_retier_is_one_commit_of_the_config_and_the_readme() {
    if !git_available() {
        return;
    }
    let tmp = committed_workspace();
    let root = tmp.path();
    let snapshot: Value =
        serde_json::from_str(&dash_committing(root, &["--json", "--retier", "--commit"])).unwrap();
    assert_eq!(snapshot["retier"][0]["applied"], true);
    let note = snapshot["notes"][0].as_str().unwrap();
    assert!(
        note.starts_with("retiered old to dormant · README updated · committed as "),
        "{note}"
    );

    assert_eq!(
        git_out(root, &["log", "-1", "--format=%s"]),
        "chore(workspace): retier old"
    );
    assert_eq!(git_out(root, &["rev-list", "--count", "HEAD"]), "2");
    let mut files: Vec<String> = git_out(root, &["show", "--name-only", "--format=", "HEAD"])
        .lines()
        .map(String::from)
        .collect();
    files.sort();
    assert_eq!(files, vec![".meta", "README.md"]);
    assert_eq!(git_out(root, &["status", "--porcelain"]), "");
    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    assert!(
        readme.contains("| [old](https://github.com/example/old) | dormant | Not touched lately |")
    );
    // Rows that did not move, and a project with no tier, are as they were.
    assert!(readme.contains("| [app](https://github.com/example/app) | active | The app |"));
    assert!(readme.contains("| lib | | No tier |"));
}

#[test]
fn a_retier_does_not_commit_over_work_under_way() {
    if !git_available() {
        return;
    }
    let tmp = committed_workspace();
    let root = tmp.path();
    let readme = root.join("README.md");
    fs::write(
        &readme,
        format!(
            "{}\nA line someone is still writing.\n",
            fs::read_to_string(&readme).unwrap()
        ),
    )
    .unwrap();

    let snapshot: Value =
        serde_json::from_str(&dash_committing(root, &["--json", "--retier", "--commit"])).unwrap();
    let note = snapshot["notes"][0].as_str().unwrap();
    assert_eq!(note, "retiered old to dormant · README updated · not committed: .meta or README.md already had changes");
    // The labels and the README are changed, the half-written line is kept, and nothing is committed.
    assert_eq!(git_out(root, &["rev-list", "--count", "HEAD"]), "1");
    let now = fs::read_to_string(&readme).unwrap();
    assert!(now.contains("| dormant |") && now.contains("A line someone is still writing."));

    // Without --commit nothing is committed either, and no reason is owed.
    let tmp = committed_workspace();
    let snapshot: Value =
        serde_json::from_str(&dash_committing(tmp.path(), &["--json", "--retier"])).unwrap();
    assert_eq!(
        snapshot["notes"][0],
        "retiered old to dormant · README updated · not committed"
    );
    assert_eq!(git_out(tmp.path(), &["rev-list", "--count", "HEAD"]), "1");
}

/// A workspace whose `app` project is a clone of a bare repository standing
/// in for a remote, with three branches there: `done` (merged into main),
/// `wip` (not merged), and main itself one commit past the merge.
fn workspace_with_remote() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    // Not canonicalized: on Windows that gives a \\?\ path, which git will not clone from.
    let root = tmp.path().to_path_buf();
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    init_repo(&seed, 0);
    run_git(&seed, 0, &["checkout", "-q", "-b", "done"]);
    run_git(
        &seed,
        0,
        &["commit", "-q", "--allow-empty", "-m", "done work"],
    );
    run_git(&seed, 0, &["checkout", "-q", "main"]);
    run_git(
        &seed,
        0,
        &["merge", "-q", "--no-ff", "done", "-m", "merge done"],
    );
    run_git(&seed, 0, &["checkout", "-q", "-b", "wip"]);
    run_git(
        &seed,
        0,
        &["commit", "-q", "--allow-empty", "-m", "unfinished"],
    );
    run_git(&seed, 0, &["checkout", "-q", "main"]);
    run_git(
        &root,
        0,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    run_git(&root, 0, &["clone", "-q", origin.to_str().unwrap(), "app"]);
    fs::write(
        root.join(".meta"),
        r#"{ "projects": { "app": "file:///origin.git" } }"#,
    )
    .unwrap();
    (tmp, root)
}

#[test]
fn only_a_merged_branch_still_where_it_was_seen_is_deleted() {
    use metarepo::plugins::dash::branches::{list, prune, Kind};
    if !git_available() {
        return;
    }
    let (_tmp, root) = workspace_with_remote();
    let origin = root.join("origin.git");
    let log = root.join("pruned.log");

    let found = list(&root, "app").unwrap();
    let told: Vec<(&str, Kind)> = found.iter().map(|b| (b.name.as_str(), b.kind)).collect();
    // No GitHub to ask: git alone vouches for what is in main, and for nothing else.
    assert_eq!(told, vec![("done", Kind::Merged), ("wip", Kind::Unknown)]);
    let done = found[0].clone();
    assert_eq!(done.ahead, 0);
    assert_eq!(found[1].ahead, 1);

    // Someone pushes to `done` after it was looked at: it must stay.
    let other = root.join("other");
    run_git(
        &root,
        0,
        &["clone", "-q", origin.to_str().unwrap(), "other"],
    );
    run_git(&other, 0, &["checkout", "-q", "done"]);
    run_git(
        &other,
        0,
        &["commit", "-q", "--allow-empty", "-m", "late work"],
    );
    run_git(&other, 0, &["push", "-q", "origin", "done"]);
    let said = prune(
        &root,
        "app",
        &[(done.name.clone(), done.sha.clone())],
        Some(&log),
    );
    assert_eq!(
        said,
        "deleted 0 remote branches of app · kept done (changed since, or refused)"
    );
    assert!(git_out(&origin, &["branch", "--list", "done"]).contains("done"));
    assert!(!log.exists());

    // Looked at again, it is no longer merged. Put it back as it was, and it goes.
    run_git(&root.join("app"), 0, &["fetch", "-q", "origin"]);
    assert!(list(&root, "app")
        .unwrap()
        .iter()
        .all(|b| b.kind != Kind::Merged));
    run_git(
        &other,
        0,
        &[
            "push",
            "-q",
            "--force",
            "origin",
            &format!("{}:refs/heads/done", done.sha),
        ],
    );
    run_git(&root.join("app"), 0, &["fetch", "-q", "origin"]);
    let said = prune(
        &root,
        "app",
        &[(done.name.clone(), done.sha.clone())],
        Some(&log),
    );
    assert!(
        said.starts_with("deleted 1 remote branch of app · logged in "),
        "{said}"
    );
    assert_eq!(git_out(&origin, &["branch", "--list", "done"]), "");
    // The unmerged branch was never touched, and the log says how to restore the other.
    assert!(git_out(&origin, &["branch", "--list", "wip"]).contains("wip"));
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged
            .trim_end()
            .ends_with(&format!(" app done {}", done.sha)),
        "{logged}"
    );
}

#[test]
fn opening_a_pull_request_refuses_what_is_not_ready_and_pushes_what_is() {
    use metarepo::plugins::dash::branches::{finish, Finish};
    if !git_available() {
        return;
    }
    let (_tmp, root) = workspace_with_remote();
    let (app, origin) = (root.join("app"), root.join("origin.git"));
    let open = Finish::OpenPr { issue: None };
    assert_eq!(
        finish(&root, "app", "main", &open, None).unwrap_err(),
        "main is the default branch"
    );
    assert_eq!(
        finish(&root, "nope", "x", &open, None).unwrap_err(),
        "no project named nope"
    );
    assert_eq!(
        finish(&root, "app", "feat/1-x", &open, None).unwrap_err(),
        "no worktree has feat/1-x checked out"
    );

    run_git(&app, 0, &["checkout", "-q", "-b", "feat/1-x"]);
    assert_eq!(
        finish(&root, "app", "feat/1-x", &open, None).unwrap_err(),
        "feat/1-x has no commits beyond main"
    );
    fs::write(app.join("new.txt"), "x").unwrap();
    assert_eq!(
        finish(&root, "app", "feat/1-x", &open, None).unwrap_err(),
        "1 uncommitted files in the worktree: commit or discard them first"
    );
    run_git(&app, 0, &["add", "new.txt"]);
    run_git(&app, 0, &["commit", "-q", "-m", "feat: x"]);
    // The push goes through; this stand-in remote is not GitHub, so it stops there and says so.
    assert_eq!(
        finish(&root, "app", "feat/1-x", &open, None).unwrap_err(),
        "pushed, but the project is not on GitHub: no pull request"
    );
    assert!(git_out(&origin, &["branch", "--list", "feat/1-x"]).contains("feat/1-x"));

    // Clearing up needs a merged pull request to vouch for the work: with none, nothing goes.
    assert_eq!(
        finish(&root, "app", "feat/1-x", &Finish::Cleanup, None).unwrap_err(),
        "no merged pull request for feat/1-x: nothing was removed"
    );
    assert!(git_out(&origin, &["branch", "--list", "feat/1-x"]).contains("feat/1-x"));
    assert_eq!(
        git_out(&app, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/1-x"
    );
}
