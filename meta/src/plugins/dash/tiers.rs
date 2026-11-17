//! Tiers: the `active`, `maintained` and `dormant` labels a workspace may give
//! its projects, and the rule that keeps them current.

use anyhow::Result;
use metarepo_core::{MetaConfig, ProjectEntry};
use serde::Serialize;
use std::path::Path;

/// Days without a commit on any branch after which an active project with no
/// work in flight moves down to maintained.
pub const QUIET_DAYS: u64 = 90;
/// Days without a commit on any branch after which a project with no work in
/// flight moves down to dormant.
pub const DORMANT_DAYS: u64 = 180;

/// A project's tier, most at work first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Active,
    Maintained,
    Dormant,
}

impl Tier {
    /// Every tier, in display order.
    pub const ALL: [Tier; 3] = [Tier::Active, Tier::Maintained, Tier::Dormant];

    /// The label that carries this tier in the workspace config.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Active => "active",
            Tier::Maintained => "maintained",
            Tier::Dormant => "dormant",
        }
    }

    /// The tier among a project's labels. Where several are present the one
    /// most at work wins; a project with none has no tier.
    pub fn of_labels(labels: &[String]) -> Option<Tier> {
        Tier::ALL
            .into_iter()
            .find(|tier| labels.iter().any(|label| label == tier.as_str()))
    }
}

/// The tier a project belongs in by what was read of it. Work in it makes it
/// active whatever it was; with none, quiet only ever moves it down. A project
/// with no tier is given none.
pub fn tier_due(tier: Option<Tier>, work: u64, quiet_days: u64) -> Option<Tier> {
    let tier = tier?;
    if work > 0 {
        return Some(Tier::Active);
    }
    if quiet_days >= DORMANT_DAYS {
        return Some(Tier::Dormant);
    }
    if quiet_days >= QUIET_DAYS && tier == Tier::Active {
        return Some(Tier::Maintained);
    }
    Some(tier)
}

/// A project whose tier label no longer matches what was read of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Move {
    pub project: String,
    pub from: Tier,
    pub to: Tier,
    /// Days since a commit on any branch.
    pub quiet_days: u64,
    /// Whether the workspace config was rewritten with the new tier.
    pub applied: bool,
}

/// Rewrite the workspace config so each moved project carries its new tier
/// label in place of the old one. The config is reloaded from disk first so
/// only what is in the file is written back. Nothing is committed.
pub fn apply(config_path: &Path, moves: &[Move]) -> Result<()> {
    if moves.is_empty() {
        return Ok(());
    }
    let mut meta = MetaConfig::load_from_file(config_path)?;
    for one in moves {
        let Some(ProjectEntry::Metadata(metadata)) = meta.projects.get_mut(&one.project) else {
            continue;
        };
        metadata
            .labels
            .retain(|label| Tier::of_labels(std::slice::from_ref(label)).is_none());
        metadata.labels.push(one.to.as_str().to_string());
        metadata.labels.sort();
        metadata.labels.dedup();
    }
    meta.save_to_file(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tier_is_read_from_labels() {
        assert_eq!(
            Tier::of_labels(&labels(&["rust", "dormant"])),
            Some(Tier::Dormant)
        );
        assert_eq!(Tier::of_labels(&labels(&["rust"])), None);
        // Two tiers on one project: the one most at work wins.
        assert_eq!(
            Tier::of_labels(&labels(&["dormant", "active"])),
            Some(Tier::Active)
        );
    }

    #[test]
    fn work_promotes_whatever_the_tier() {
        assert_eq!(tier_due(Some(Tier::Dormant), 1, 400), Some(Tier::Active));
        assert_eq!(tier_due(Some(Tier::Maintained), 3, 0), Some(Tier::Active));
    }

    #[test]
    fn quiet_only_moves_down() {
        assert_eq!(tier_due(Some(Tier::Active), 0, 89), Some(Tier::Active));
        assert_eq!(tier_due(Some(Tier::Active), 0, 90), Some(Tier::Maintained));
        assert_eq!(tier_due(Some(Tier::Active), 0, 180), Some(Tier::Dormant));
        assert_eq!(
            tier_due(Some(Tier::Maintained), 0, 150),
            Some(Tier::Maintained)
        );
        assert_eq!(
            tier_due(Some(Tier::Maintained), 0, 180),
            Some(Tier::Dormant)
        );
        // A dormant project with a recent commit but no work in flight stays put.
        assert_eq!(tier_due(Some(Tier::Dormant), 0, 2), Some(Tier::Dormant));
    }

    #[test]
    fn untiered_projects_are_left_alone() {
        assert_eq!(tier_due(None, 5, 500), None);
    }
}
