//! Report what an add/remove changed in the workspace and offer to commit
//! the `.meta` / `.gitignore` updates.

use anyhow::Result;
use colored::Colorize;
use metarepo_core::{is_interactive, prompt_confirm, NonInteractiveMode};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Snapshot {
    base: PathBuf,
    files: Vec<(PathBuf, Option<String>)>,
    project_dir: PathBuf,
    dir_existed: bool,
}

impl Snapshot {
    pub fn take(base: &Path, meta_file: &Path, project: &str) -> Self {
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let gitignore = base.join(".gitignore");
        let files = vec![
            (meta_file.to_path_buf(), read(meta_file)),
            (gitignore.clone(), read(&gitignore)),
        ];
        let project_dir = base.join(project);
        Self {
            base: base.to_path_buf(),
            files,
            dir_existed: project_dir.symlink_metadata().is_ok(),
            project_dir,
        }
    }

    /// Print what changed; returns the changed workspace files.
    pub fn report(&self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        let mut lines = Vec::new();
        for (path, before) in &self.files {
            let after = std::fs::read_to_string(path).ok();
            if *before == after {
                continue;
            }
            let name = path
                .strip_prefix(&self.base)
                .unwrap_or(path)
                .display()
                .to_string();
            let verb = match (before, &after) {
                (None, Some(_)) => "Created",
                (Some(_), None) => "Deleted",
                _ => "Modified",
            };
            lines.push(format!("{} {}", verb.yellow(), name.bright_white()));
            let b = before.as_deref().unwrap_or("");
            let a = after.as_deref().unwrap_or("");
            let old: Vec<&str> = b.lines().collect();
            let new: Vec<&str> = a.lines().collect();
            let mut shown = 0;
            for l in new.iter().filter(|l| !old.contains(l)) {
                if shown < 6 {
                    lines.push(format!("    {}", format!("+ {}", l.trim()).green()));
                }
                shown += 1;
            }
            for l in old.iter().filter(|l| !new.contains(l)) {
                if shown < 6 {
                    lines.push(format!("    {}", format!("- {}", l.trim()).red()));
                }
                shown += 1;
            }
            if shown > 6 {
                lines.push(format!(
                    "    {}",
                    format!("... {} more lines", shown - 6).dimmed()
                ));
            }
            changed.push(path.clone());
        }
        let exists = self.project_dir.symlink_metadata().is_ok();
        let rel = self
            .project_dir
            .strip_prefix(&self.base)
            .unwrap_or(&self.project_dir)
            .display()
            .to_string();
        match (self.dir_existed, exists) {
            (false, true) => lines.push(format!("{} {}", "Created".yellow(), rel)),
            (true, false) => lines.push(format!("{} {}", "Deleted".yellow(), rel)),
            _ => {}
        }
        if !lines.is_empty() {
            println!("  {}", "Changes made:".bold());
            for l in lines {
                println!("     {}", l);
            }
        }
        changed
    }

    /// Report, then commit the changed workspace files if wanted.
    /// `commit`: Some(true) always, Some(false) never, None = ask in a TTY.
    pub fn finish(
        &self,
        action: &str,
        project: &str,
        commit: Option<bool>,
        non_interactive: NonInteractiveMode,
    ) -> Result<()> {
        let changed = self.report();
        if changed.is_empty() || commit == Some(false) || !in_git_repo(&self.base) {
            return Ok(());
        }
        let names: Vec<String> = changed
            .iter()
            .map(|p| {
                p.strip_prefix(&self.base)
                    .unwrap_or(p)
                    .display()
                    .to_string()
            })
            .collect();
        let want = match commit {
            Some(true) => true,
            _ if is_interactive() => prompt_confirm(
                &format!("Commit {}?", names.join(" and ")),
                true,
                non_interactive,
            )?,
            _ => {
                println!(
                    "     {}",
                    format!(
                        "Not committed. Run: git add {} && git commit",
                        names.join(" ")
                    )
                    .dimmed()
                );
                false
            }
        };
        if !want {
            return Ok(());
        }
        let msg = format!("chore(meta): {} project {}", action, project);
        match git_commit(&self.base, &changed, &msg) {
            Ok(()) => println!("     {} {}", "Committed:".green(), msg),
            Err(e) => eprintln!("  {} commit failed: {}", "⚠".yellow(), e),
        }
        Ok(())
    }
}

fn in_git_repo(base: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(base)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_commit(base: &Path, files: &[PathBuf], msg: &str) -> Result<()> {
    let run = |args: &[&str]| -> Result<()> {
        let out = Command::new("git")
            .arg("-C")
            .arg(base)
            .args(args)
            .args(files)
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    // Pathspec-limited so other staged work is never swept into this commit.
    run(&["add", "--"])?;
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(["commit", "-m", msg, "--"])
        .args(files)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    #[test]
    fn reports_and_commits_only_workspace_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        git(d, &["init", "-q"]);
        git(d, &["config", "user.name", "t"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "commit.gpgsign", "false"]);
        std::fs::write(d.join(".meta"), "{}\n").unwrap();
        std::fs::write(d.join("other.txt"), "x").unwrap();
        git(d, &["add", ".meta"]);
        git(d, &["commit", "-qm", "init"]);

        let snap = Snapshot::take(d, &d.join(".meta"), "proj");
        std::fs::write(d.join(".meta"), "{\"projects\":1}\n").unwrap();
        std::fs::write(d.join(".gitignore"), "proj\n").unwrap();
        std::fs::write(d.join("other.txt"), "changed").unwrap();
        git(d, &["add", "other.txt"]);

        snap.finish("add", "proj", Some(true), NonInteractiveMode::Defaults)
            .unwrap();
        assert!(git(d, &["log", "-1", "--format=%s"]).contains("add project proj"));
        let files = git(d, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(files.contains(".meta") && files.contains(".gitignore"));
        assert!(!files.contains("other.txt"), "staged unrelated file leaked");
    }

    #[test]
    fn no_commit_flag_leaves_tree_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        git(d, &["init", "-q"]);
        std::fs::write(d.join(".meta"), "{}\n").unwrap();
        let snap = Snapshot::take(d, &d.join(".meta"), "p");
        std::fs::write(d.join(".meta"), "{\"a\":1}\n").unwrap();
        snap.finish("add", "p", Some(false), NonInteractiveMode::Defaults)
            .unwrap();
        assert!(git(d, &["log", "--oneline"]).is_empty());
    }
}
