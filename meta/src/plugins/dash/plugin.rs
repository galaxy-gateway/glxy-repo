//! Plugin wiring for `meta dash`.

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command};
use metarepo_core::{BasePlugin, MetaPlugin, RuntimeConfig};
use std::io::IsTerminal;

use super::{gather, Options, ProjectState, Section, Snapshot, Tier};

/// Registers the top-level `meta dash` command.
pub struct DashPlugin;

impl DashPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DashPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl MetaPlugin for DashPlugin {
    fn name(&self) -> &str {
        "dash"
    }

    fn register_commands(&self, app: Command) -> Command {
        app.subcommand(
            Command::new("dash")
                .about("Workspace overview: tiers, branches in flight, issues and Claude sessions")
                .version(env!("CARGO_PKG_VERSION"))
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Print everything that was read as JSON"),
                )
                .arg(
                    Arg::new("summary")
                        .long("summary")
                        .action(ArgAction::SetTrue)
                        .help("Print the plain summary even in a terminal"),
                )
                .arg(
                    Arg::new("no-github")
                        .long("no-github")
                        .action(ArgAction::SetTrue)
                        .help("Do not ask GitHub for pull requests and issues"),
                )
                .arg(
                    Arg::new("no-sessions")
                        .long("no-sessions")
                        .action(ArgAction::SetTrue)
                        .help("Do not list Claude Code sessions"),
                )
                .arg(
                    Arg::new("no-cost")
                        .long("no-cost")
                        .action(ArgAction::SetTrue)
                        .help("Do not add up cost from Claude Code transcripts"),
                )
                .arg(
                    Arg::new("retier")
                        .long("retier")
                        .action(ArgAction::SetTrue)
                        .help("Apply the tier moves that are due: the labels, and the README's Tier column"),
                )
                .arg(
                    Arg::new("commit")
                        .long("commit")
                        .action(ArgAction::SetTrue)
                        .requires("retier")
                        .help("With --retier: commit the config and README change (never pushes)"),
                )
                .after_long_help(metarepo_core::format_help_description(
                    "Read the whole workspace once and report it: each project's tier and git\n\
                     state, the branches in flight with their pull requests and checks, open\n\
                     issues, and the Claude Code sessions running in it. The read covers every\n\
                     project whatever directory it is run from; --label and --exclude-label\n\
                     narrow it.\n\
                     \n\
                     Tiers are the labels active, maintained and dormant. A project with work\n\
                     in flight (unpushed commits, extra worktrees, recently committed branches)\n\
                     is due active; one with none is due maintained after 90 days without a\n\
                     commit on any branch and dormant after 180. Moves are only reported unless\n\
                     --retier is given. --retier also brings the Tier column of the README's project\n\
                     tables up to date, and with --commit makes one commit of the two files.\n\
                     \n\
                     Pull requests and issues are asked through the gh CLI, and sessions through\n\
                     the claude CLI; each is skipped with a note when its CLI is missing. Where most\n\
                     projects come from one GitHub account, its repositories are compared with the\n\
                     workspace: ones not here, and projects here that are archived or gone there.\n\
                     Cost is\n\
                     summed from the session transcripts Claude Code keeps on this machine, by\n\
                     project, day and model, at the prices listed in this version of meta.\n\
                     \n\
                     Examples:\n  \
                       meta dash                         Interactive dashboard (summary when piped)\n  \
                       meta dash --summary               Summary by tier\n  \
                       meta dash --json                  Everything, as JSON\n  \
                       meta dash --json --no-github      Local state only, no network\n  \
                       meta dash --retier --commit       Apply the tier moves that are due, as one commit\n",
                )),
        )
    }

    fn handle_command(&self, matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
        // In a terminal, with nothing asking for text or a write: the interface.
        let wants_text =
            matches.get_flag("json") || matches.get_flag("summary") || matches.get_flag("retier");
        if !wants_text && std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
            return super::tui::run(config);
        }
        let snapshot = gather(
            config,
            Options {
                github: !matches.get_flag("no-github"),
                sessions: !matches.get_flag("no-sessions"),
                retier: matches.get_flag("retier"),
                cost: !matches.get_flag("no-cost"),
                commit: matches.get_flag("commit"),
            },
        )?;
        if matches.get_flag("json") {
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        } else {
            print!("{}", summary(&snapshot));
        }
        Ok(())
    }
}

impl BasePlugin for DashPlugin {
    fn version(&self) -> Option<&str> {
        Some(env!("CARGO_PKG_VERSION"))
    }

    fn author(&self) -> Option<&str> {
        Some("Metarepo Contributors")
    }

    fn description(&self) -> Option<&str> {
        Some("Workspace overview: tiers, branches in flight, issues and Claude sessions")
    }
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// The snapshot as a few plain lines: projects by tier, then the totals.
pub fn summary(snapshot: &Snapshot) -> String {
    let mut out = format!("{}\n", snapshot.root);
    let width = snapshot
        .projects
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(0);
    let mut groups: Vec<(String, Option<Tier>)> = Tier::ALL
        .into_iter()
        .map(|tier| {
            let name = tier.as_str();
            (
                format!("{}{}", name[..1].to_uppercase(), &name[1..]),
                Some(tier),
            )
        })
        .collect();
    groups.push((
        if snapshot.has_tiers {
            "No tier"
        } else {
            "Projects"
        }
        .to_string(),
        None,
    ));
    for (title, tier) in groups {
        let own: Vec<_> = snapshot
            .projects
            .iter()
            .filter(|p| p.tier == tier)
            .collect();
        if own.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{title} ({})\n", own.len()));
        for project in own {
            let state = match project.state {
                ProjectState::Missing => "(missing)".to_string(),
                ProjectState::NotGit => "(not a git repo)".to_string(),
                ProjectState::NoWorktree => "(no main worktree)".to_string(),
                ProjectState::Ok => {
                    let mut parts = vec![project.branch.clone()];
                    if project.ahead > 0 {
                        parts.push(format!("+{}", project.ahead));
                    }
                    if project.behind > 0 {
                        parts.push(format!("-{}", project.behind));
                    }
                    if project.dirty > 0 {
                        parts.push(format!("*{}", project.dirty));
                    }
                    let flying = snapshot
                        .board
                        .iter()
                        .filter(|i| i.project == project.name)
                        .count();
                    if flying > 0 {
                        parts.push(format!("· {flying} in flight"));
                    }
                    let open = snapshot
                        .issues
                        .iter()
                        .filter(|i| i.project == project.name)
                        .count();
                    if open > 0 {
                        parts.push(format!("· {}", plural(open, "issue")));
                    }
                    parts.join(" ")
                }
            };
            out.push_str(&format!("  {:<width$}  {state}\n", project.name));
        }
    }

    let count = |section: Section| {
        snapshot
            .board
            .iter()
            .filter(|i| i.section == section)
            .count()
    };
    let mut board = vec![format!("{} in flight", snapshot.board.len())];
    for (section, words) in [
        (Section::Attention, "waiting on you"),
        (Section::Fix, "need fixing"),
        (Section::Working, "with an agent working"),
        (Section::Local, "not pushed"),
        (Section::Ready, "ready"),
    ] {
        if count(section) > 0 {
            board.push(format!("{} {words}", count(section)));
        }
    }
    out.push_str(&format!("\nBoard     {}\n", board.join(" · ")));
    if snapshot.github.asked {
        out.push_str(&format!("Issues    {} open\n", snapshot.issues.len()));
    }
    let asking = snapshot.sessions.iter().filter(|s| s.needs_you).count();
    let busy = snapshot
        .sessions
        .iter()
        .filter(|s| s.status == "busy")
        .count();
    out.push_str(&format!(
        "Sessions  {} · {busy} busy · {asking} waiting on you\n",
        snapshot.sessions.len()
    ));
    if let Some(cost) = &snapshot.cost {
        out.push_str(&format!(
            "Cost      today ${:.2} · 7 days ${:.2} · 30 days ${:.2} · all ${:.2}\n",
            cost.usd_over(Some(1)),
            cost.usd_over(Some(7)),
            cost.usd_over(Some(30)),
            cost.usd_over(None)
        ));
        if !cost.unpriced.is_empty() {
            out.push_str(&format!(
                "No price listed for: {}\n",
                cost.unpriced.join(", ")
            ));
        }
    }
    for one in &snapshot.retier {
        let (from, to) = (one.from.as_str(), one.to.as_str());
        out.push_str(&if one.applied {
            format!("Retiered  {} {from} to {to}\n", one.project)
        } else {
            let why = if one.to == Tier::Active {
                "it has work in flight".to_string()
            } else {
                format!("{} quiet days", one.quiet_days)
            };
            format!(
                "Tier due  {} {from} to {to}: {why} (apply with --retier)\n",
                one.project
            )
        });
    }
    if let Some(drift) = snapshot.drift.as_ref().filter(|drift| !drift.is_empty()) {
        for (list, words) in [
            (&drift.missing, format!("In {} but not here", drift.org)),
            (&drift.archived, "Archived on GitHub".to_string()),
            (&drift.gone, format!("Not in {} under that name", drift.org)),
        ] {
            if !list.is_empty() {
                out.push_str(&format!("Drift     {words}: {}\n", list.join(", ")));
            }
        }
    }
    if !snapshot.github.failed.is_empty() {
        out.push_str(&format!(
            "No GitHub data for: {}\n",
            snapshot.github.failed.join(", ")
        ));
    }
    for note in &snapshot.notes {
        out.push_str(&format!("{note}\n"));
    }
    out
}
