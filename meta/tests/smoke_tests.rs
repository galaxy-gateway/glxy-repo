// End-to-end smoke tests: one happy path per core command, driving the real
// `meta` binary against temp directories and local git repos (no network).
//
// Every invocation runs with a timeout, and with stdin closed unless the test
// says otherwise, so a stray interactive prompt fails the test instead of
// hanging the suite. These are deliberately shallow: they catch a command that
// is broken outright, not every flag.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const META_BIN: &str = env!("CARGO_BIN_EXE_meta");
const TIMEOUT: Duration = Duration::from_secs(60);

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

/// Write a `.meta` listing `(project, url)` pairs, plus a `hello` script.
fn write_meta(dir: &Path, projects: &[(&str, &str)]) {
    let entries: Vec<String> = projects
        .iter()
        .map(|(name, url)| format!("    {:?}: {:?}", name, url))
        .collect();
    fs::write(
        dir.join(".meta"),
        format!(
            "{{\n  \"projects\": {{\n{}\n  }},\n  \"scripts\": {{ \"hello\": \"touch script-ran\" }}\n}}\n",
            entries.join(",\n")
        ),
    )
    .unwrap();
}

fn file_url(path: &Path) -> String {
    // Windows canonical paths look like \\?\C:\dir; git wants file:///C:/dir.
    let p = path.canonicalize().unwrap().display().to_string();
    let p = p.trim_start_matches(r"\\?\").replace('\\', "/");
    format!("file://{}{}", if p.starts_with('/') { "" } else { "/" }, p)
}

struct Run {
    ok: bool,
    /// stdout followed by stderr.
    text: String,
}

/// Run `meta` in `dir`, failing the test if it outlives TIMEOUT. `HOME` points
/// at an empty directory so the developer's global git and meta configuration
/// cannot leak in. With `Stdio::piped()` the write end of stdin stays open and
/// unwritten until meta exits.
fn meta_with_stdin(dir: &Path, args: &[&str], stdin: Stdio) -> Run {
    let home = TempDir::new().unwrap();
    let mut child = Command::new(META_BIN)
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn meta binary");

    // Drain the pipes on threads so a chatty child cannot block on a full pipe
    // while we poll for exit.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let read_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let read_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = format!("{}{}", read_out.join().unwrap(), read_err.join().unwrap());
    match status {
        Some(status) => Run {
            ok: status.success(),
            text,
        },
        None => panic!(
            "meta {:?} still running after {:?} (waiting on a prompt?); output so far:\n{}",
            args, TIMEOUT, text
        ),
    }
}

/// Run `meta` with stdin closed.
fn meta_in(dir: &Path, args: &[&str]) -> Run {
    meta_with_stdin(dir, args, Stdio::null())
}

/// Run `meta` with stdin closed and require a zero exit status.
fn meta_ok(dir: &Path, args: &[&str]) -> String {
    let run = meta_in(dir, args);
    assert!(run.ok, "meta {:?} failed:\n{}", args, run.text);
    run.text
}

/// Workspace with two local repos, `alpha` and `beta`, listed in `.meta`.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for name in ["alpha", "beta"] {
        init_repo(&tmp.path().join(name));
    }
    write_meta(
        tmp.path(),
        &[
            ("alpha", "https://example.com/alpha.git"),
            ("beta", "https://example.com/beta.git"),
        ],
    );
    tmp
}

/// A bare repo with one commit on `main`, usable as a `file://` remote.
fn bare_remote(parent: &Path, name: &str) -> String {
    let seed = parent.join(format!("{name}-seed"));
    init_repo(&seed);
    let bare = parent.join(format!("{name}.git"));
    let status = Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(&seed)
        .arg(&bare)
        .status()
        .unwrap();
    assert!(status.success(), "git clone --bare failed");
    file_url(&bare)
}

/// Add a worktree for `branch` in both workspace projects, merged with main.
fn add_worktree_everywhere(ws: &Path, branch: &str) {
    meta_ok(ws, &["worktree", "add", branch, "--all", "--from", "HEAD"]);
    for name in ["alpha", "beta"] {
        assert!(
            ws.join(name).join(".worktrees").join(branch).is_dir(),
            "{name} has no worktree for {branch}"
        );
    }
}

#[test]
fn init_creates_workspace_config() {
    let tmp = TempDir::new().unwrap();
    let out = meta_ok(tmp.path(), &["init"]);
    // A fresh init writes the config as .metarepo (.meta is the legacy name).
    assert!(tmp.path().join(".metarepo").is_file(), "got:\n{out}");
}

#[test]
fn repo_add_list_remove() {
    if !git_available() {
        return;
    }
    let remotes = TempDir::new().unwrap();
    let url = bare_remote(remotes.path(), "lib");
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    meta_ok(ws, &["init"]);

    meta_ok(ws, &["repo", "add", "lib", &url, "--no-commit"]);
    assert!(ws.join("lib/.git").exists(), "repo add did not clone");

    let listed = meta_ok(ws, &["repo", "list", "--minimal"]);
    assert_eq!(listed.trim(), "lib");

    meta_ok(ws, &["repo", "remove", "lib", "--force", "--no-commit"]);
    let listed = meta_ok(ws, &["repo", "list", "--minimal"]);
    assert!(!listed.contains("lib"), "still listed:\n{listed}");
}

#[test]
fn git_clone_fetches_meta_repo_and_children() {
    if !git_available() {
        return;
    }
    let remotes = TempDir::new().unwrap();
    let child_url = bare_remote(remotes.path(), "child");

    // The meta repo itself: a repo whose committed .meta lists the child.
    let seed = remotes.path().join("meta-seed");
    init_repo(&seed);
    write_meta(&seed, &[("child", &child_url)]);
    run_git(&seed, &["add", ".meta"]);
    run_git(&seed, &["commit", "-q", "-m", "add manifest"]);

    let dest = TempDir::new().unwrap();
    meta_ok(dest.path(), &["git", "clone", &file_url(&seed)]);
    let cloned = dest.path().join("meta-seed");
    assert!(cloned.join(".meta").is_file(), "meta repo not cloned");
    assert!(cloned.join("child/.git").exists(), "child repo not cloned");
}

#[test]
fn git_status_reports_each_repo() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let out = meta_ok(tmp.path(), &["git", "status"]);
    assert!(out.contains("alpha") && out.contains("beta"), "got:\n{out}");
}

#[test]
fn git_update_clones_missing_repos() {
    if !git_available() {
        return;
    }
    let remotes = TempDir::new().unwrap();
    let url = bare_remote(remotes.path(), "lib");
    let tmp = TempDir::new().unwrap();
    write_meta(tmp.path(), &[("lib", &url)]);

    meta_ok(tmp.path(), &["git", "update"]);
    assert!(tmp.path().join("lib/.git").exists(), "lib was not cloned");
}

#[test]
fn exec_runs_command_in_each_repo() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    meta_ok(tmp.path(), &["exec", "--all", "--", "touch", "exec-ran"]);
    for name in ["alpha", "beta"] {
        assert!(tmp.path().join(name).join("exec-ran").is_file());
    }
}

#[test]
fn run_executes_named_script() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let out = meta_ok(tmp.path(), &["run", "hello", "--all"]);
    for name in ["alpha", "beta"] {
        assert!(
            tmp.path().join(name).join("script-ran").is_file(),
            "script did not run in {name}:\n{out}"
        );
    }
}

#[test]
fn worktree_add_list_remove() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let ws = tmp.path();
    add_worktree_everywhere(ws, "feature");

    let listed = meta_ok(ws, &["worktree", "list"]);
    assert!(listed.contains("feature"), "got:\n{listed}");

    meta_ok(ws, &["worktree", "remove", "feature", "--all"]);
    for name in ["alpha", "beta"] {
        assert!(
            !ws.join(name).join(".worktrees/feature").exists(),
            "{name} worktree not removed"
        );
    }
}

#[test]
fn worktree_clean_removes_merged_worktrees() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let ws = tmp.path();
    add_worktree_everywhere(ws, "landed");

    meta_ok(ws, &["worktree", "clean", "--yes"]);
    for name in ["alpha", "beta"] {
        assert!(
            !ws.join(name).join(".worktrees/landed").exists(),
            "{name} merged worktree not cleaned"
        );
    }
}

/// Regression for #195: a new branch with no --from reached a prompt that read
/// stdin directly, so `--non-interactive` hung whenever stdin was an open pipe.
/// The pipe here is held open and never written; a prompt trips the timeout.
#[test]
fn worktree_add_new_branch_non_interactive_never_reads_stdin() {
    if !git_available() {
        return;
    }
    let tmp = workspace();
    let ws = tmp.path();

    // Stdio::piped() leaves the write end with the test, which never writes.
    let defaults = meta_with_stdin(
        ws,
        &[
            "--non-interactive",
            "defaults",
            "wt",
            "add",
            "fresh",
            "--all",
        ],
        Stdio::piped(),
    );
    let fail = meta_with_stdin(
        ws,
        &["--non-interactive", "fail", "wt", "add", "strict", "--all"],
        Stdio::piped(),
    );

    assert!(defaults.ok, "defaults mode failed:\n{}", defaults.text);
    for name in ["alpha", "beta"] {
        assert!(
            ws.join(name).join(".worktrees/fresh").is_dir(),
            "{name}: defaults mode did not create the worktree from HEAD"
        );
        assert!(
            !ws.join(name).join(".worktrees/strict").exists(),
            "{name}: fail mode created a worktree"
        );
    }
    assert!(
        !fail.ok && fail.text.contains("--from"),
        "fail mode should exit non-zero and name --from:\n{}",
        fail.text
    );
}
