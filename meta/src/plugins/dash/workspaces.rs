//! Other workspaces on this machine: finding them, and loading one to read.

use anyhow::{anyhow, Result};
use metarepo_core::{MetaConfig, ProjectEntry, RuntimeConfig};
use std::path::{Path, PathBuf};

use super::tiers::Tier;

/// A workspace found on this machine, as its config alone describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    /// The config file that makes it a workspace.
    pub config: PathBuf,
    pub name: String,
    pub projects: usize,
    /// Projects in each tier, in [`Tier::ALL`] order.
    pub tiers: [usize; 3],
}

/// What the config directly inside `root` says of the workspace there.
pub fn describe(root: &Path) -> Option<Workspace> {
    // Canonical, so the same workspace reached two ways is one workspace.
    let root = &root.canonicalize().ok()?;
    let found = MetaConfig::config_in_dir(root)?;
    let config = MetaConfig::load_from_file(&found.path).ok()?;
    let mut tiers = [0; 3];
    for entry in config.projects.values() {
        if let ProjectEntry::Metadata(metadata) = entry {
            if let Some(tier) = Tier::of_labels(&metadata.labels) {
                tiers[Tier::ALL.iter().position(|one| *one == tier).unwrap_or(0)] += 1;
            }
        }
    }
    Some(Workspace {
        root: root.to_path_buf(),
        config: found.path,
        name: root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned()),
        projects: config.projects.len(),
        tiers,
    })
}

/// Hands over every workspace within three folders of `home` as it is found.
/// Hidden folders, `Library` and `node_modules` are not looked into. A home
/// folder can hold slow places (network and cloud folders), so this can take
/// minutes: it reports as it goes for whoever is waiting.
pub fn find(home: &Path, mut on_found: impl FnMut(Workspace)) {
    walkdir::WalkDir::new(home)
        .max_depth(3)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            entry.file_type().is_dir()
                && (entry.depth() == 0
                    || !(name.starts_with('.') || name == "Library" || name == "node_modules"))
        })
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| describe(entry.path()))
        .for_each(&mut on_found);
}

/// A runtime config for the workspace whose config file is `config_path`, as
/// if `meta -w` had been run at its root.
pub fn config_at(config_path: &Path) -> Result<RuntimeConfig> {
    let root = config_path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", config_path.display()))?;
    Ok(RuntimeConfig {
        meta_config: MetaConfig::load_from_file(config_path)?,
        working_dir: root.to_path_buf(),
        meta_file_path: Some(config_path.to_path_buf()),
        experimental: false,
        non_interactive: None,
        scope_workspace: true,
        settings_catalog: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(dir: &Path, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(".meta"), body).unwrap();
    }

    #[test]
    fn workspaces_are_found_near_home_with_their_tiers() {
        let home = tempfile::TempDir::new().unwrap();
        workspace(
            &home.path().join("Projects/one"),
            r#"{"projects":{"a":{"url":"u","labels":["active"]},"b":{"url":"u","labels":["dormant","x"]},"c":"u"}}"#,
        );
        workspace(&home.path().join("two"), r#"{"projects":{}}"#);
        // Not looked into: hidden, too deep, and node_modules.
        workspace(&home.path().join(".hidden/ws"), r#"{"projects":{}}"#);
        workspace(&home.path().join("a/b/c/d"), r#"{"projects":{}}"#);
        workspace(&home.path().join("node_modules/ws"), r#"{"projects":{}}"#);

        let mut found = Vec::new();
        find(home.path(), |one| found.push(one));
        found.sort_by(|a, b| a.root.cmp(&b.root));
        let names: Vec<&str> = found.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["one", "two"]);
        assert_eq!(found[0].projects, 3);
        assert_eq!(found[0].tiers, [1, 0, 1]);

        let config = config_at(&found[0].config).unwrap();
        assert_eq!(config.meta_root().as_deref(), Some(found[0].root.as_path()));
        assert!(config.scope_workspace);
    }
}
