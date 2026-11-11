//! `meta workspace` (alias `ws`): commands that act on workspaces themselves
//! rather than on the repos inside them.

use crate::plugins::init::InitPlugin;
use crate::plugins::project::init_child_workspace;
use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use metarepo_core::{MetaConfig, MetaPlugin, RuntimeConfig};

pub struct WorkspacePlugin;

impl WorkspacePlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkspacePlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// In-scope projects that are themselves meta workspaces (have their own config).
fn nested_workspaces(config: &RuntimeConfig) -> Vec<String> {
    let Some(root) = config.meta_root() else {
        return Vec::new();
    };
    config
        .scoped_project_keys()
        .into_iter()
        .filter(|key| MetaConfig::config_in_dir(&root.join(key)).is_some())
        .collect()
}

impl MetaPlugin for WorkspacePlugin {
    fn name(&self) -> &str {
        "workspace"
    }

    fn register_commands(&self, app: Command) -> Command {
        // Reuse `meta init` wholesale so `workspace init` takes the same flags.
        let ws = InitPlugin::new()
            .register_commands(Command::new("workspace"))
            .about("Create and list workspaces, including nested ones")
            .visible_alias("ws")
            .version(env!("CARGO_PKG_VERSION"))
            .after_long_help(metarepo_core::format_help_description(
                "A workspace is a directory with a meta config. A nested workspace is a\n\
                 workspace registered as a project of an enclosing one, so a team can run\n\
                 fleet commands at its own level while the outer workspace still reaches\n\
                 everything with --root.\n\
                 \n\
                 Examples:\n  \
                   meta workspace init                 initialize the current directory\n  \
                   meta ws init teams/payments         create and register a nested workspace\n  \
                   meta ws list                        list the nested workspaces in scope\n  \
                   meta exec --projects \"$(meta ws list | paste -sd, -)\" -- git status",
            ))
            .mut_subcommand("init", |c| {
                c.about("Initialize this directory, or a nested child workspace by name")
                    .arg(Arg::new("name").help(
                        "Relative path of a nested child workspace to create and register here. \
                         Omit to initialize the current directory (same as meta init)",
                    ))
            })
            .subcommand(
                Command::new("list")
                    .visible_alias("ls")
                    .version(env!("CARGO_PKG_VERSION"))
                    .about("List the nested workspaces in scope, one per line"),
            );
        app.subcommand(ws)
    }

    fn handle_command(&self, matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
        match matches.subcommand() {
            Some(("init", m)) => match m.get_one::<String>("name") {
                Some(name) => {
                    let base = config
                        .meta_root()
                        .unwrap_or_else(|| config.working_dir.clone());
                    init_child_workspace(name, &base)
                }
                None => InitPlugin::new().handle_command(m, config),
            },
            Some(("list", _)) => {
                let nested = nested_workspaces(config);
                if nested.is_empty() {
                    eprintln!(
                        "No nested workspaces here. Create one with: meta workspace init <name>"
                    );
                }
                for name in nested {
                    println!("{name}");
                }
                Ok(())
            }
            _ => {
                let app = self.register_commands(Command::new("meta"));
                if let Some(ws) = app.find_subcommand("workspace") {
                    metarepo_core::with_standard_help(ws.clone().bin_name("meta workspace"))
                        .print_help()?;
                }
                Ok(())
            }
        }
    }
}
