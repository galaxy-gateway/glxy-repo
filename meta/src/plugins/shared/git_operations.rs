use anyhow::{Context, Result};
use colored::*;
use git2::{Cred, FetchOptions, RemoteCallbacks, Repository};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Once;

static CANCELLED: AtomicBool = AtomicBool::new(false);
static CLONING: AtomicBool = AtomicBool::new(false);
static HANDLER: Once = Once::new();
static RECV_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static TOTAL_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static RECV_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Live clone progress: (received objects, total objects, received bytes).
pub fn clone_progress() -> (usize, usize, usize) {
    (
        RECV_OBJECTS.load(Ordering::Relaxed),
        TOTAL_OBJECTS.load(Ordering::Relaxed),
        RECV_BYTES.load(Ordering::Relaxed),
    )
}

/// Human text for the spinner: "1200/5400 objects, 3.1 MB" or "connecting".
pub fn clone_progress_text() -> String {
    let (r, t, b) = clone_progress();
    if t == 0 {
        "connecting".to_string()
    } else {
        format!("{}/{} objects, {}", r, t, super::spinner::format_bytes(b))
    }
}

/// Removes a project directory this run created unless `finish()` is called.
/// While alive, Ctrl+C sets a flag that aborts the clone (via the libgit2
/// progress callback) so the error path runs and Drop cleans up; outside a
/// guard, Ctrl+C exits immediately as usual.
pub struct CloneGuard {
    path: Option<PathBuf>,
}

impl CloneGuard {
    /// `path` must not have existed before this run.
    pub fn new(path: &Path) -> Self {
        HANDLER.call_once(|| {
            let _ = ctrlc::set_handler(|| {
                if CLONING.load(Ordering::SeqCst) {
                    CANCELLED.store(true, Ordering::SeqCst);
                } else {
                    std::process::exit(130);
                }
            });
        });
        CANCELLED.store(false, Ordering::SeqCst);
        CLONING.store(true, Ordering::SeqCst);
        for a in [&RECV_OBJECTS, &TOTAL_OBJECTS, &RECV_BYTES] {
            a.store(0, Ordering::Relaxed);
        }
        Self {
            path: Some(path.to_path_buf()),
        }
    }

    /// Clone succeeded: keep the directory and restore default Ctrl+C.
    pub fn finish(&mut self) {
        self.path = None;
        CLONING.store(false, Ordering::SeqCst);
    }
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        CLONING.store(false, Ordering::SeqCst);
        if let Some(p) = self.path.take() {
            if p.exists() && std::fs::remove_dir_all(&p).is_ok() {
                eprintln!("  Removed partial clone at {}", p.display());
            }
        }
    }
}

fn cancel_aware(callbacks: &mut RemoteCallbacks<'_>) {
    callbacks.transfer_progress(|p| {
        RECV_OBJECTS.store(p.received_objects(), Ordering::Relaxed);
        TOTAL_OBJECTS.store(p.total_objects(), Ordering::Relaxed);
        RECV_BYTES.store(p.received_bytes(), Ordering::Relaxed);
        !CANCELLED.load(Ordering::SeqCst)
    });
}

fn clone_error(e: git2::Error) -> anyhow::Error {
    if CANCELLED.load(Ordering::SeqCst) {
        anyhow::anyhow!("Clone cancelled by user")
    } else {
        anyhow::anyhow!("Failed to clone repository: {}", e)
    }
}

/// Parse and validate a `--depth <N>` CLI argument value into a shallow-clone
/// depth, shared by `meta git clone` and `meta repo add`.
///
/// Returns `Ok(None)` when `raw` is `None` (no `--depth` given). Returns an
/// error when the value fails to parse as an integer or is not a positive
/// integer, so callers surface a clear message before attempting any clone.
pub fn parse_depth_arg(raw: Option<&String>) -> Result<Option<i32>> {
    match raw {
        Some(s) => {
            let d: i32 = s.parse().map_err(|_| {
                anyhow::anyhow!("Invalid --depth value '{}': must be a positive integer", s)
            })?;
            if d <= 0 {
                return Err(anyhow::anyhow!(
                    "Invalid --depth value {}: depth must be a positive integer",
                    d
                ));
            }
            Ok(Some(d))
        }
        None => Ok(None),
    }
}

/// Re-truncate a shallow repository's history to `depth` commits from the
/// current remote tips by running `git fetch --depth=<N>`.
///
/// A plain `git fetch`/`pull` on a shallow repository accumulates every new
/// commit and never moves the shallow boundary, so history grows unboundedly
/// over time. Fetching with an explicit `--depth` moves the boundary up to
/// the new tips; commits behind it become unreachable once local branches
/// advance (and are eventually pruned by gc).
///
/// Shells out to the `git` CLI rather than libgit2 because libgit2 rejects
/// shallow fetches on some transports (e.g. local/file) that the CLI accepts.
pub fn refetch_shallow(repo_path: &Path, depth: i32) -> Result<()> {
    if depth <= 0 {
        return Err(anyhow::anyhow!(
            "Invalid fetch depth {}: depth must be a positive integer",
            depth
        ));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("fetch")
        .arg(format!("--depth={}", depth))
        .output()
        .context("Failed to run git fetch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "git fetch --depth={} failed: {}",
            depth,
            stderr.trim()
        ));
    }

    Ok(())
}

/// Clone a repository with authentication support.
///
/// `depth` optionally requests a shallow clone with the given history depth.
/// A value of `Some(d)` with `d <= 0` is rejected.
pub fn clone_with_auth(
    url: &str,
    path: &Path,
    bare: bool,
    depth: Option<i32>,
) -> Result<Repository> {
    if let Some(d) = depth {
        if d <= 0 {
            return Err(anyhow::anyhow!(
                "Invalid clone depth {}: depth must be a positive integer",
                d
            ));
        }
    }

    // Check if this is an SSH URL
    if url.starts_with("git@") || url.starts_with("ssh://") {
        // Set up authentication callbacks for SSH
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_url, username_from_url, allowed_types| {
            // Get the username (default to "git" for GitHub/GitLab/etc)
            let username = username_from_url.unwrap_or("git");

            // If SSH agent is requested, try it first
            if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                // Try to find SSH keys in standard locations
                if let Ok(home) = std::env::var("HOME") {
                    let ssh_dir = Path::new(&home).join(".ssh");

                    // Try common SSH key names in order of preference
                    let key_names = ["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"];

                    for key_name in &key_names {
                        let private_key = ssh_dir.join(key_name);
                        if private_key.exists() {
                            // Check if there's a public key as well
                            let public_key = ssh_dir.join(format!("{}.pub", key_name));
                            let public_key_path = if public_key.exists() {
                                Some(public_key.as_path())
                            } else {
                                None
                            };

                            if let Ok(cred) = Cred::ssh_key(
                                username,
                                public_key_path,
                                private_key.as_path(),
                                None, // No passphrase for now
                            ) {
                                return Ok(cred);
                            }
                        }
                    }
                }

                // Try SSH agent as fallback
                if let Ok(cred) = Cred::ssh_key_from_agent(username) {
                    return Ok(cred);
                }
            }

            // If we couldn't authenticate, return an error
            Err(git2::Error::from_str(
                "SSH authentication failed. Please ensure your SSH keys are set up correctly.",
            ))
        });

        cancel_aware(&mut callbacks);
        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);
        if let Some(d) = depth {
            fetch_options.depth(d);
        }

        // Build the repository with authentication
        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fetch_options);

        if bare {
            builder.bare(true);
        }

        // Clone the repository
        builder.clone(url, path).map_err(|e| {
            if CANCELLED.load(Ordering::SeqCst) {
                clone_error(e)
            } else if e.to_string().contains("authentication") || e.to_string().contains("SSH") {
                anyhow::anyhow!("SSH authentication failed. Please ensure:\n  1. Your SSH key is set up correctly (~/.ssh/id_ed25519 or ~/.ssh/id_rsa)\n  2. The key is added to your GitHub/GitLab account\n  3. You have access to the repository\n\nOriginal error: {}", e)
            } else {
                anyhow::anyhow!("Failed to clone repository: {}", e)
            }
        })
    } else {
        // For HTTPS URLs, use RepoBuilder without authentication callbacks so we
        // can still apply a shallow-clone depth when requested.
        let mut callbacks = RemoteCallbacks::new();
        cancel_aware(&mut callbacks);
        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);
        if let Some(d) = depth {
            fetch_options.depth(d);
        }

        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fetch_options);
        if bare {
            builder.bare(true);
        }

        builder.clone(url, path).map_err(clone_error)
    }
}

/// Create a default worktree for a bare repository
pub fn create_default_worktree(bare_repo_path: &Path, project_path: &Path) -> Result<()> {
    // Try to detect the default branch
    let default_branch = detect_default_branch(bare_repo_path)?;

    // Create worktree at <project>/<default-branch>/
    let worktree_path = project_path.join(&default_branch);

    let output = Command::new("git")
        .arg("-C")
        .arg(bare_repo_path)
        .arg("worktree")
        .arg("add")
        .arg(&worktree_path)
        .arg(&default_branch)
        .output()
        .context("Failed to create default worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "Failed to create default worktree: {}",
            stderr
        ));
    }

    println!(
        "     {} {}",
        "✅".green(),
        format!("Created default worktree: {}", worktree_path.display()).green()
    );

    Ok(())
}

/// Detect the default branch of a repository
pub fn detect_default_branch(repo_path: &Path) -> Result<String> {
    // Try to get the default branch from the remote HEAD
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("symbolic-ref")
        .arg("refs/remotes/origin/HEAD")
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Output format: "refs/remotes/origin/main"
            if let Some(branch) = stdout.trim().strip_prefix("refs/remotes/origin/") {
                return Ok(branch.to_string());
            }
        }
    }

    // Fallback: try common default branch names
    for branch in &["main", "master", "develop"] {
        let check_output = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .arg("rev-parse")
            .arg("--verify")
            .arg(format!("refs/remotes/origin/{}", branch))
            .output();

        if let Ok(output) = check_output {
            if output.status.success() {
                return Ok(branch.to_string());
            }
        }
    }

    // Ultimate fallback
    Ok("main".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_guard_removes_dir_unless_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        let kept = tmp.path().join("kept");
        std::fs::create_dir(&gone).unwrap();
        std::fs::create_dir(&kept).unwrap();
        drop(CloneGuard::new(&gone));
        let mut g = CloneGuard::new(&kept);
        g.finish();
        drop(g);
        assert!(!gone.exists());
        assert!(kept.exists());
    }

    #[test]
    fn clone_with_auth_rejects_non_positive_depth() {
        // A dummy, unreachable URL and path — the depth validation must fail
        // before any network access or filesystem work is attempted.
        let path = Path::new("/nonexistent/does-not-matter");

        let err = match clone_with_auth("https://example.invalid/repo.git", path, false, Some(0)) {
            Err(e) => e,
            Ok(_) => panic!("depth of 0 must be rejected"),
        };
        assert_eq!(
            err.to_string(),
            "Invalid clone depth 0: depth must be a positive integer"
        );

        let err = match clone_with_auth("https://example.invalid/repo.git", path, false, Some(-1)) {
            Err(e) => e,
            Ok(_) => panic!("negative depth must be rejected"),
        };
        assert_eq!(
            err.to_string(),
            "Invalid clone depth -1: depth must be a positive integer"
        );
    }

    #[test]
    fn parse_depth_arg_none_when_flag_absent() {
        assert_eq!(parse_depth_arg(None).unwrap(), None);
    }

    #[test]
    fn parse_depth_arg_accepts_positive_integer() {
        let raw = "1".to_string();
        assert_eq!(parse_depth_arg(Some(&raw)).unwrap(), Some(1));

        let raw = "42".to_string();
        assert_eq!(parse_depth_arg(Some(&raw)).unwrap(), Some(42));
    }

    #[test]
    fn parse_depth_arg_rejects_zero_and_negative() {
        let raw = "0".to_string();
        let err = parse_depth_arg(Some(&raw)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid --depth value 0: depth must be a positive integer"
        );

        let raw = "-3".to_string();
        let err = parse_depth_arg(Some(&raw)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid --depth value -3: depth must be a positive integer"
        );
    }

    #[test]
    fn parse_depth_arg_rejects_non_numeric() {
        let raw = "abc".to_string();
        let err = parse_depth_arg(Some(&raw)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid --depth value 'abc': must be a positive integer"
        );
    }
}
