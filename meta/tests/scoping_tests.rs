// End-to-end tests for directory-aware command scoping.
//
// These drive the real `meta` binary with `current_dir` set to different points
// in a workspace and assert that multi-project commands act only on the in-scope
// projects, that `--workspace` forces whole-workspace scope, and that `--root`
// resolves the outermost metarepo for a metarepo-inside-a-metarepo.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

const META_BIN: &str = env!("CARGO_BIN_EXE_meta");

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test"])
        .args(["-c", "user.email=test@example.com"])
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {}", args, e));
    assert!(status.success(), "git {:?} failed", args);
}

/// Create a git repo at `dir` with one commit on `main`.
fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    run_git(dir, &["init", "-q", "-b", "main"]);
    run_git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
}

fn write_meta(dir: &Path, projects: &[&str]) {
    let entries: Vec<String> = projects
        .iter()
        .map(|p| format!("    {:?}: \"https://example.com/{}.git\"", p, p))
        .collect();
    fs::write(
        dir.join(".meta"),
        format!(
            "{{\n  \"projects\": {{\n{}\n  }}\n}}\n",
            entries.join(",\n")
        ),
    )
    .unwrap();
}

fn meta_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(META_BIN)
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run meta binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Workspace with `app`, `plugins/a`, `plugins/b`.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    for key in ["app", "plugins/a", "plugins/b"] {
        init_repo(&ws.join(key));
    }
    write_meta(ws, &["app", "plugins/a", "plugins/b"]);
    tmp
}

#[test]
fn project_list_scopes_to_subdirectory() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let out = meta_in(
        &tmp.path().join("plugins"),
        &["project", "list", "--minimal"],
    );
    let s = stdout(&out);
    assert!(
        s.contains("plugins/a") && s.contains("plugins/b"),
        "got:\n{s}"
    );
    assert!(!s.contains("app"), "out-of-scope project listed:\n{s}");
}

#[test]
fn repo_is_the_command_and_project_its_alias() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let repo = stdout(&meta_in(tmp.path(), &["repo", "list", "--minimal"]));
    let project = stdout(&meta_in(tmp.path(), &["project", "list", "--minimal"]));
    assert!(
        repo.contains("app") && repo.contains("plugins/a"),
        "got:\n{repo}"
    );
    assert_eq!(repo, project);
    let help = stdout(&meta_in(tmp.path(), &["repo", "--help"]));
    assert!(help.contains("meta repo"), "got:\n{help}");
}

#[test]
fn ws_and_wt_aliases_and_nested_workspace_init() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let wt = stdout(&meta_in(tmp.path(), &["wt", "--help"]));
    assert!(wt.contains("meta worktree"), "got:\n{wt}");

    assert!(stdout(&meta_in(tmp.path(), &["ws", "list"]))
        .trim()
        .is_empty());
    let init = meta_in(tmp.path(), &["ws", "init", "teams/payments"]);
    assert!(init.status.success(), "ws init failed: {init:?}");
    let listed = stdout(&meta_in(tmp.path(), &["workspace", "list"]));
    assert_eq!(listed.trim(), "teams/payments");
}

#[test]
fn labels_select_repos_across_commands() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let add = meta_in(
        tmp.path(),
        &["repo", "label", "add", "app", "backend", "rust"],
    );
    assert!(add.status.success(), "label add failed: {add:?}");
    assert_eq!(stdout(&add).trim(), "app: backend, rust");

    let list = |args: &[&str]| {
        let mut full = args.to_vec();
        full.extend(["repo", "list", "--minimal"]);
        stdout(&meta_in(tmp.path(), &full))
    };
    assert_eq!(list(&["--label", "backend"]).trim(), "app");
    assert_eq!(
        list(&["--label", "backend", "--label", "rust"]).trim(),
        "app"
    );
    assert_eq!(list(&["--label", "backend", "--label", "web"]).trim(), "");
    assert_eq!(list(&["--label", "web,rust"]).trim(), "app");
    let rest = list(&["--exclude-label", "backend"]);
    assert!(
        !rest.contains("app") && rest.contains("plugins/a"),
        "got:\n{rest}"
    );

    // Commands that reload the config from disk honor the selection too.
    let status = stdout(&meta_in(
        tmp.path(),
        &["--label", "backend", "git", "status"],
    ));
    assert!(
        status.contains("app:") && !status.contains("plugins/a:"),
        "got:\n{status}"
    );
    let exec = meta_in(
        tmp.path(),
        &[
            "--label",
            "backend",
            "exec",
            "--all",
            "--include-disabled",
            "--",
            "pwd",
        ],
    );
    let exec = format!("{}{}", stdout(&exec), String::from_utf8_lossy(&exec.stderr));
    assert!(
        exec.contains("app") && !exec.contains("plugins/a"),
        "got:\n{exec}"
    );

    let removed = meta_in(
        tmp.path(),
        &["repo", "label", "rm", "app", "rust", "backend"],
    );
    assert_eq!(stdout(&removed).trim(), "app: (none)");
    let reserved = meta_in(tmp.path(), &["repo", "label", "add", "app", "@workspace"]);
    assert!(!reserved.status.success());
}

#[test]
fn project_list_at_root_shows_all() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let s = stdout(&meta_in(tmp.path(), &["project", "list", "--minimal"]));
    assert!(s.contains("app") && s.contains("plugins/a") && s.contains("plugins/b"));
}

#[test]
fn workspace_flag_overrides_subdirectory_scope() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let s = stdout(&meta_in(
        &tmp.path().join("plugins"),
        &["--workspace", "project", "list", "--minimal"],
    ));
    assert!(
        s.contains("app"),
        "--workspace should include all projects:\n{s}"
    );
}

#[test]
fn git_status_scopes_to_current_project_without_main() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let s = stdout(&meta_in(&tmp.path().join("app"), &["git", "status"]));
    assert!(s.contains("app:"), "got:\n{s}");
    assert!(!s.contains("plugins/a"), "out-of-scope project shown:\n{s}");
    assert!(
        !s.contains("Main repository"),
        "main repo should not show when scoped to a project:\n{s}"
    );
}

#[test]
fn root_flag_targets_outermost_metarepo() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path();
    init_repo(&outer.join("outer-proj"));
    init_repo(&outer.join("inner").join("inner-proj"));
    write_meta(outer, &["outer-proj", "inner"]);
    write_meta(&outer.join("inner"), &["inner-proj"]);

    // Default from inner/: nearest metarepo wins → inner-proj.
    let nearest = stdout(&meta_in(
        &outer.join("inner"),
        &["project", "list", "--minimal"],
    ));
    assert!(nearest.contains("inner-proj"), "nearest got:\n{nearest}");
    assert!(!nearest.contains("outer-proj"), "nearest got:\n{nearest}");

    // --root --workspace from inner/: outermost metarepo, all its projects.
    let rooted = stdout(&meta_in(
        &outer.join("inner"),
        &["--root", "--workspace", "project", "list", "--minimal"],
    ));
    assert!(
        rooted.contains("outer-proj") && rooted.contains("inner"),
        "rooted got:\n{rooted}"
    );
}
