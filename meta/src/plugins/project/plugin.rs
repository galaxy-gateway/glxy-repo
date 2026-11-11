use super::changes::Snapshot;
use super::{
    check_workspace, convert_to_bare, import_project_recursive_with_options,
    import_project_with_options, init_child_workspace, list_projects, list_projects_minimal,
    locate_workspace_config, remove_project, rename_project, show_project_tree, update_projects,
};
use crate::plugins::shared::parse_depth_arg;
use anyhow::Result;
use clap::ArgMatches;
use colored::Colorize;
use metarepo_core::{
    arg, command, is_interactive, plugin, prompt_select, prompt_text, prompt_url, BasePlugin,
    MetaPlugin, NonInteractiveMode, RuntimeConfig,
};

/// ProjectPlugin using the new simplified plugin architecture
pub struct ProjectPlugin;

impl ProjectPlugin {
    pub fn new() -> Self {
        Self
    }

    /// Create the plugin using the builder pattern
    pub fn create_plugin() -> impl MetaPlugin {
        plugin("repo")
            .version(env!("CARGO_PKG_VERSION"))
            .description("Project management operations")
            .author("Metarepo Contributors")
            .help_description(
                "Manage the set of repositories tracked in a workspace's .meta file.\n\
                 \n\
                 A project is an entry under \"projects\" in .meta, mapping a local path\n\
                 to a git URL (or the literal \"local\" for an in-tree directory). These\n\
                 commands add, list, update, and remove those entries, keeping the .meta\n\
                 file and the working tree in sync.\n\
                 \n\
                 Common workflows:\n\
                 \n\
                   meta repo add web https://github.com/acme/web.git   clone and track\n\
                   meta repo list --flat                               show every project\n\
                   meta repo update --recursive                        sync nested repos\n\
                 \n\
                 Directories named in the .meta \"ignore\" list (.git, target,\n\
                 node_modules, ...) are skipped during discovery.",
            )
            .command(
                command("add")
                    .about("Add a project to the workspace")
                    .long_about("Add a project to the workspace.\n\n\
                                 This command can:\n\
                                 • Clone a new repository from a URL\n\
                                 • Import an existing local repository\n\
                                 • Create a symlink to an external directory\n\
                                 • Auto-detect repository URLs from existing directories\n\
                                 • Recursively import nested meta repositories\n\n\
                                 Examples:\n\
                                   meta repo add myproject https://github.com/user/repo.git  # Clone new\n\
                                   meta repo add myproject ../external-repo                   # Symlink\n\
                                   meta repo add myproject                                    # Use existing")
                    .aliases(vec!["import".to_string(), "i".to_string(), "a".to_string()])
                    .help_description(
                        "Track a repository under a workspace path in .meta.\n\
                         \n\
                         Adds an entry to the \"projects\" map in .meta and reconciles the\n\
                         working tree with it. The source argument decides the mode: a git\n\
                         URL is cloned into <path>; an external local path that is a git repo\n\
                         is symlinked in and its remote recorded as external:<url>; an\n\
                         existing in-tree directory is adopted as-is (recording its remote, or\n\
                         local: when it has none). With no source you are prompted in a TTY.\n\
                         \n\
                         By default clones use the bare-with-worktrees layout (disable per\n\
                         workspace via default_bare); pass --bare to force it. Use --depth to\n\
                         perform a shallow git clone (the depth is recorded so re-clones via\n\
                         meta git update stay shallow); this does not apply to recursive\n\
                         imports. Use --init-git to git init a plain directory before tracking\n\
                         it. Use --recursive\n\
                         (with --max-depth, --flatten) to import nested meta repositories, or\n\
                         --no-recursive to override a workspace that enables it by default. If\n\
                         the added repo declares itself a meta module, you are shown it and,\n\
                         in a TTY, offered to enable it.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo add web https://github.com/acme/web.git   clone and track\n\
                           meta repo add libs ../shared-libs                   symlink an external repo\n\
                           meta repo add docs                                  adopt an existing directory\n\
                           meta repo add mono URL --recursive --flatten        import nested repos flat",
                    )
                    .with_help_formatting()
                    .arg(
                        arg("path")
                            .help("Local directory name for the project")
                            .required(false)
                            .takes_value(true)
                    )
                    .arg(
                        arg("source")
                            .help("Git URL or path to external directory (optional)")
                            .required(false)
                            .takes_value(true)
                    )
                    .arg(
                        arg("recursive")
                            .long("recursive")
                            .short('r')
                            .help("Recursively import nested meta repositories")
                    )
                    .arg(
                        arg("max-depth")
                            .long("max-depth")
                            .help("Maximum depth for recursive imports (default: 3)")
                            .takes_value(true)
                    )
                    .arg(
                        arg("flatten")
                            .long("flatten")
                            .help("Import nested projects at root level instead of maintaining hierarchy")
                    )
                    .arg(
                        arg("no-recursive")
                            .long("no-recursive")
                            .help("Disable recursive import even if configured in .meta")
                    )
                    .arg(
                        arg("init-git")
                            .long("init-git")
                            .help("Automatically initialize git repository if directory is not a git repo")
                    )
                    .arg(
                        arg("bare")
                            .long("bare")
                            .help("Clone as bare repository with worktree structure")
                    )
                    .arg(
                        arg("depth")
                            .long("depth")
                            .help("Git shallow clone depth (limits history fetched when cloning)")
                            .takes_value(true)
                    )
                    .arg(
                        arg("yes")
                            .long("yes")
                            .short('y')
                            .help("Skip the interactive option review before adding")
                    )
                    .arg(
                        arg("commit")
                            .long("commit")
                            .help("Commit the .meta and .gitignore updates without asking")
                    )
                    .arg(
                        arg("no-commit")
                            .long("no-commit")
                            .help("Do not offer to commit the .meta and .gitignore updates")
                    )
            )
            .command(
                command("list")
                    .about("List all projects in the workspace (tree view by default)")
                    .help_description(
                        "List the projects tracked in the current workspace.\n\
                         \n\
                         Reads the \"projects\" map from .meta and prints the entries that\n\
                         fall within the directory-aware scope (running inside a subtree\n\
                         narrows the listing). The default is a tree view; --flat prints a\n\
                         detailed list with each project's URL and on-disk status (present,\n\
                         missing, or symlink), and --minimal prints just the names, one per\n\
                         line, for scripting.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo list             tree view of the workspace\n\
                           meta repo list --flat       names with URLs and status\n\
                           meta repo list --minimal    bare names for scripts",
                    )
                    .with_help_formatting()
                    .aliases(vec!["ls".to_string(), "l".to_string()])
                    .arg(
                        arg("flat")
                            .long("flat")
                            .short('f')
                            .help("Display projects as a flat list with details")
                    )
                    .arg(
                        arg("minimal")
                            .long("minimal")
                            .short('m')
                            .help("Display only project names (minimal output)")
                    )
            )
            .command(
                command("tree")
                    .about("Display the project hierarchy as a tree")
                    .help_description(
                        "Show the workspace projects as an indented hierarchy.\n\
                         \n\
                         Equivalent to \"meta repo list\" in its default mode, rendering the\n\
                         scoped \"projects\" entries as a tree that reflects nested paths. The\n\
                         same overrides apply: --flat switches to a detailed list with URLs\n\
                         and on-disk status, and --minimal prints only project names.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo tree            hierarchy of tracked projects\n\
                           meta repo tree --flat      flat list with details",
                    )
                    .with_help_formatting()
                    .arg(
                        arg("flat")
                            .long("flat")
                            .short('f')
                            .help("Display projects as a flat list with details")
                    )
                    .arg(
                        arg("minimal")
                            .long("minimal")
                            .short('m')
                            .help("Display only project names (minimal output)")
                    )
            )
            .command(
                command("update")
                    .about("Update all projects by pulling the latest changes")
                    .help_description(
                        "Pull the latest changes for every tracked project.\n\
                         \n\
                         Walks the \"projects\" map and runs a fetch-and-pull on each repo that\n\
                         exists on disk. Projects whose directory is missing or that are not\n\
                         git repositories are skipped with a note, and per-repo failures are\n\
                         reported without aborting the rest. A per-project summary table is\n\
                         printed at the end.\n\
                         \n\
                         With --recursive, any updated project that is itself a meta workspace\n\
                         has its nested projects updated too, down to --depth levels (default\n\
                         3). Aliased as \"pull\".\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo update                 pull every project\n\
                           meta repo update --recursive      also update nested workspaces",
                    )
                    .aliases(vec!["pull".to_string()])
                    .with_help_formatting()
                    .arg(
                        arg("recursive")
                            .long("recursive")
                            .short('r')
                            .help("Also update nested repositories")
                    )
                    .arg(
                        arg("depth")
                            .long("depth")
                            .help("Maximum depth for recursive updates (default: 3)")
                            .takes_value(true)
                    )
            )
            .command(
                command("remove")
                    .about("Remove a project from the workspace")
                    .help_description(
                        "Stop tracking a project and remove its .meta entry.\n\
                         \n\
                         Deletes the named project from the \"projects\" map in .meta. With no\n\
                         name you are prompted to pick one in a TTY. Before removing, the\n\
                         working tree is checked for uncommitted changes (across all worktrees\n\
                         for bare repos); if any are found the command refuses unless --force\n\
                         is given. Plain --remove only edits .meta and leaves files in place;\n\
                         --force additionally deletes the project directory from disk.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo remove web              untrack web (keep files)\n\
                           meta repo remove web --force       untrack and delete the directory",
                    )
                    .aliases(vec!["rm".to_string(), "r".to_string()])
                    .with_help_formatting()
                    .arg(
                        arg("name")
                            .help("Name of the project to remove")
                            .required(false)
                            .takes_value(true)
                    )
                    .arg(
                        arg("force")
                            .long("force")
                            .short('f')
                            .help("Force removal even with uncommitted changes, and delete directory")
                    )
                    .arg(
                        arg("commit")
                            .long("commit")
                            .help("Commit the .meta and .gitignore updates without asking")
                    )
                    .arg(
                        arg("no-commit")
                            .long("no-commit")
                            .help("Do not offer to commit the .meta and .gitignore updates")
                    )
            )
            .command(
                command("rename")
                    .about("Rename a project and move its directory")
                    .help_description(
                        "Rename a tracked project and move its on-disk directory.\n\
                         \n\
                         Re-keys the project's entry in the \"projects\" map from <old_name> to\n\
                         <new_name> and renames its directory to match. Fails if the new name\n\
                         is already tracked or the target directory already exists. For real\n\
                         git repositories (not symlinks) the working tree is checked first and\n\
                         the rename is refused when there are uncommitted changes; commit or\n\
                         stash them before retrying. Aliased as \"mv\" and \"move\".\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo rename web frontend     rename web to frontend",
                    )
                    .aliases(vec!["mv".to_string(), "move".to_string()])
                    .with_help_formatting()
                    .arg(
                        arg("old_name")
                            .help("Current name of the project")
                            .required(true)
                            .takes_value(true)
                    )
                    .arg(
                        arg("new_name")
                            .help("New name for the project")
                            .required(true)
                            .takes_value(true)
                    )
            )
            .command(
                command("convert-to-bare")
                    .about("Convert a normal repository to a bare repo with worktrees")
                    .help_description(
                        "Convert a tracked repository to the bare-with-worktrees layout.\n\
                         \n\
                         Rewrites a normal project clone into a bare repository whose checked\n\
                         out branches live as worktrees, the same layout meta uses for new\n\
                         clones by default. The project must exist on disk, be a git\n\
                         repository, and not already be bare. If it is already configured as\n\
                         bare the command reports that and exits without changes.\n\
                         \n\
                         Use this to migrate older flat clones so they work with meta's\n\
                         worktree commands.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo convert-to-bare web     migrate web to bare layout",
                    )
                    .with_help_formatting()
                    .arg(
                        arg("project")
                            .help("Name of the project to convert")
                            .required(true)
                            .takes_value(true)
                    )
            )
            .command(
                command("init")
                    .about("Initialize a nested child workspace and register it in the parent")
                    .help_description(
                        "Create a child metarepo under the current workspace and register it.\n\
                         \n\
                         Makes a directory <name> inside the workspace root, gives it its own\n\
                         .meta via the standard init path, and adds it to the parent config as a\n\
                         tracked local project. The child config inherits shared defaults from\n\
                         the enclosing .meta chain and overrides only what it needs, so settings\n\
                         and global scripts set once at the top flow down to nested repos.\n\
                         \n\
                         The name must be a relative path inside the workspace (no absolute\n\
                         paths and no .. escapes). Refuses to clobber an existing child config.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo init services/api    scaffold a nested workspace",
                    )
                    .with_help_formatting()
                    .arg(
                        arg("name")
                            .help("Relative path of the child workspace to create")
                            .required(true)
                            .takes_value(true),
                    ),
            )
            .command(
                command("check")
                    .about("Report (and optionally fix) workspace drift")
                    .aliases(vec!["ck".to_string()])
                    .help_description(
                        "Check the workspace for drift between the config and the working tree.\n\
                         \n\
                         Runs a set of hygiene checks and prints a report. By default it is a\n\
                         dry run and exits non-zero when any drift is found, so it works as a CI\n\
                         or pre-commit lint. Pass --fix to apply the fixable corrections.\n\
                         \n\
                         Checks:\n  \
                           - .gitignore missing an entry for a remote-backed project (fixable)\n  \
                           - a tracked project whose directory is missing on disk (report)\n  \
                           - a top-level git repo on disk not tracked in the config (report)\n\
                         \n\
                         Stale .gitignore lines are reported context permitting but never\n\
                         auto-removed, since a former project entry cannot be told apart from a\n\
                         hand-added ignore.\n\
                         \n\
                         Examples:\n\
                         \n\
                           meta repo check           report drift, non-zero exit if any\n\
                           meta repo check --fix      apply the fixable corrections",
                    )
                    .with_help_formatting()
                    .arg(
                        arg("fix")
                            .long("fix")
                            .help("Apply the fixable corrections instead of only reporting"),
                    ),
            )
            .handler("add", handle_add)
            .handler("list", handle_list)
            .handler("tree", handle_tree)
            .handler("update", handle_update)
            .handler("remove", handle_remove)
            .handler("rename", handle_rename)
            .handler("convert-to-bare", handle_convert_to_bare)
            .handler("init", handle_init)
            .handler("check", handle_check)
            .build()
    }
}

/// --commit => Some(true), --no-commit => Some(false), neither => ask.
fn commit_choice(matches: &ArgMatches) -> Option<bool> {
    if matches.get_flag("no-commit") {
        Some(false)
    } else if matches.get_flag("commit") {
        Some(true)
    } else {
        None
    }
}

struct AddChoices {
    path: String,
    bare: bool,
    depth: Option<i32>,
}

/// Ask for layout and depth (unless set by flag), then loop on a summary with
/// Proceed / Edit / Cancel. Returns None when the user cancels.
fn review_add_options(
    base_path: &std::path::Path,
    mut c: AddChoices,
    source: Option<&str>,
    recursive: bool,
    bare_locked: bool,
    depth_locked: bool,
    ni: NonInteractiveMode,
) -> Result<Option<AddChoices>> {
    let clonable = source.is_some_and(metarepo_core::is_supported_git_url);
    let ask = |c: &mut AddChoices, with_path: bool| -> Result<()> {
        if with_path {
            loop {
                let p = prompt_text(
                    "Project path (relative to workspace)",
                    Some(&c.path),
                    false,
                    ni,
                )?;
                match metarepo_core::validate_path_segment("project name", &p) {
                    Ok(()) => {
                        c.path = p;
                        break;
                    }
                    Err(e) => println!("  {} {}", "x".red(), e),
                }
            }
        }
        if clonable && !bare_locked {
            let items = vec!["Bare repository + worktree", "Regular clone"];
            let pick = prompt_select("Layout", items, Some(if c.bare { 0 } else { 1 }), ni)?;
            c.bare = pick.starts_with("Bare");
        }
        if clonable && !depth_locked && !recursive {
            let pick = prompt_select(
                "History",
                vec!["Full history", "Shallow clone"],
                Some(if c.depth.is_some() { 1 } else { 0 }),
                ni,
            )?;
            c.depth = if pick.starts_with("Shallow") {
                let raw = prompt_text("Depth (commits)", Some("1"), false, ni)?;
                match parse_depth_arg(Some(&raw)) {
                    Ok(d) => d,
                    Err(e) => {
                        println!("  {} {} - using full history", "!".yellow(), e);
                        None
                    }
                }
            } else {
                None
            };
        }
        Ok(())
    };

    ask(&mut c, false)?;
    loop {
        let target = base_path.join(&c.path);
        println!("\n  {}", "New project".cyan().bold());
        println!("     Workspace: {}", base_path.display());
        println!(
            "     Path:      {}{}",
            target.display(),
            if target.exists() {
                "  (already exists)"
            } else {
                ""
            }
        );
        println!(
            "     Source:    {}",
            source.unwrap_or("(existing directory)")
        );
        if clonable {
            println!(
                "     Layout:    {}{}{}",
                if c.bare {
                    "bare + worktree"
                } else {
                    "regular clone"
                },
                c.depth
                    .map(|d| format!(", depth {}", d))
                    .unwrap_or_default(),
                if recursive { ", recursive" } else { "" }
            );
        }
        let pick = prompt_select(
            "Next step",
            vec!["Proceed", "Edit options", "Cancel"],
            Some(0),
            ni,
        )?;
        match pick.as_str() {
            "Proceed" => return Ok(Some(c)),
            "Edit options" => ask(&mut c, true)?,
            _ => return Ok(None),
        }
    }
}

/// Handler for the add command
fn handle_add(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let non_interactive = config
        .non_interactive
        .unwrap_or(NonInteractiveMode::Defaults);

    // Get or prompt for the project path
    let path = match matches.get_one::<String>("path") {
        Some(p) => p.clone(),
        None => {
            if is_interactive() {
                println!(
                    "\n  📋 {}",
                    "Add a new project to your workspace".cyan().bold()
                );
                prompt_text("Project name/path", None, false, non_interactive)?
            } else {
                return Err(anyhow::anyhow!(
                    "Project path is required. Use 'meta repo add <path>' or run interactively in a terminal"
                ));
            }
        }
    };

    // Get or prompt for the source URL
    let source_opt = match matches.get_one::<String>("source") {
        Some(s) => Some(s.clone()),
        None => {
            if is_interactive() {
                prompt_url("Repository URL or path", None, false, non_interactive)?
            } else {
                None
            }
        }
    };
    let source = source_opt.as_deref();

    let init_git = matches.get_flag("init-git");
    let bare = matches.get_flag("bare");
    let clone_depth = parse_depth_arg(matches.get_one::<String>("depth"))?;

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    // Check for recursive import flags
    let recursive = matches.get_flag("recursive");
    let no_recursive = matches.get_flag("no-recursive");
    let flatten = matches.get_flag("flatten");
    let max_depth = matches
        .get_one::<String>("max-depth")
        .and_then(|s| s.parse::<usize>().ok());

    // Determine if we should use recursive import
    let use_recursive = if no_recursive {
        false // Explicitly disabled
    } else if recursive || flatten || max_depth.is_some() {
        true // Explicitly enabled or has related flags
    } else {
        // Check configuration or global default
        config
            .meta_config
            .nested
            .as_ref()
            .map(|n| n.recursive_import)
            .unwrap_or(false)
    };

    // Determine if we should use bare repository
    let use_bare = if bare {
        true // Explicitly enabled via flag
    } else {
        // Check global default (defaults to true for bare repos)
        config.meta_config.default_bare.unwrap_or(true)
    };

    let (path, use_bare, clone_depth) = if is_interactive() && !matches.get_flag("yes") {
        let start = AddChoices {
            path,
            bare: use_bare,
            depth: clone_depth,
        };
        match review_add_options(
            &base_path,
            start,
            source,
            use_recursive,
            bare,
            matches.get_one::<String>("depth").is_some(),
            non_interactive,
        )? {
            Some(c) => (c.path, c.bare, c.depth),
            None => {
                println!("  Cancelled, nothing was changed");
                return Ok(());
            }
        }
    } else {
        (path, use_bare, clone_depth)
    };

    let snapshot = locate_workspace_config(&base_path)
        .ok()
        .map(|meta_file| Snapshot::take(&base_path, &meta_file, &path));

    if use_recursive || flatten || max_depth.is_some() {
        if clone_depth.is_some() {
            eprintln!(
                "  {} --depth is ignored for recursive imports; nested projects are cloned in full",
                "⚠".yellow()
            );
        }
        import_project_recursive_with_options(
            &path,
            source,
            &base_path,
            use_recursive,
            max_depth,
            flatten,
            init_git,
            use_bare,
        )?;
    } else {
        import_project_with_options(&path, source, &base_path, init_git, use_bare, clone_depth)?;
    }

    // If the added repo declares itself a meta module, surface it (and, in a
    // TTY, offer to enable it). Activation is always explicit.
    let repo_root = base_path.join(&path);
    crate::plugins::module::offer_enable_after_add(&repo_root, config, non_interactive);

    if let Some(snap) = snapshot {
        snap.finish("add", &path, commit_choice(matches), non_interactive)?;
    }

    Ok(())
}

/// Handler for the list command
fn handle_list(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let base_path = config
        .meta_root()
        .unwrap_or_else(|| config.working_dir.clone());
    let scope = config.scoped_project_keys();

    // Check flags for output format
    if matches.get_flag("minimal") {
        // Minimal: just project names
        list_projects_minimal(&base_path, &scope)?;
    } else if matches.get_flag("flat") {
        // Flat: list with details
        list_projects(&base_path, &scope)?;
    } else {
        // Default: tree view
        show_project_tree(&base_path, &scope)?;
    }
    Ok(())
}

/// Handler for the tree command
fn handle_tree(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let base_path = config
        .meta_root()
        .unwrap_or_else(|| config.working_dir.clone());
    let scope = config.scoped_project_keys();

    // Check flags for output format (same as list command)
    if matches.get_flag("minimal") {
        // Minimal: just project names
        list_projects_minimal(&base_path, &scope)?;
    } else if matches.get_flag("flat") {
        // Flat: list with details
        list_projects(&base_path, &scope)?;
    } else {
        // Default: tree view
        show_project_tree(&base_path, &scope)?;
    }
    Ok(())
}

/// Handler for the update command
fn handle_update(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    let recursive = matches.get_flag("recursive");
    let depth = matches
        .get_one::<String>("depth")
        .and_then(|s| s.parse::<usize>().ok());

    update_projects(&base_path, recursive, depth)?;
    Ok(())
}

/// Handler for the remove command
fn handle_remove(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let non_interactive = config
        .non_interactive
        .unwrap_or(NonInteractiveMode::Defaults);

    // Get or prompt for project name
    let name = match matches.get_one::<String>("name") {
        Some(n) => n.clone(),
        None => {
            if is_interactive() {
                let project_names: Vec<String> =
                    config.meta_config.projects.keys().cloned().collect();

                if project_names.is_empty() {
                    return Err(anyhow::anyhow!("No projects found in workspace"));
                }

                println!(
                    "\n  🗑️  {}",
                    "Remove a project from workspace".cyan().bold()
                );
                prompt_select("Project to remove", project_names, None, non_interactive)?
            } else {
                return Err(anyhow::anyhow!(
                    "Project name is required. Use 'meta repo remove <name>' or run interactively in a terminal"
                ));
            }
        }
    };

    let force = matches.get_flag("force");

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    let snapshot = locate_workspace_config(&base_path)
        .ok()
        .map(|meta_file| Snapshot::take(&base_path, &meta_file, &name));
    remove_project(&name, &base_path, force)?;
    if let Some(snap) = snapshot {
        snap.finish("remove", &name, commit_choice(matches), non_interactive)?;
    }
    Ok(())
}

/// Handler for the rename command
fn handle_rename(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let old_name = matches.get_one::<String>("old_name").unwrap();
    let new_name = matches.get_one::<String>("new_name").unwrap();

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    rename_project(old_name, new_name, &base_path)?;
    Ok(())
}

/// Handler for the convert-to-bare command
fn handle_convert_to_bare(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let project = matches.get_one::<String>("project").unwrap();

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    convert_to_bare(project, &base_path)?;
    Ok(())
}

/// Handler for the init command: scaffold a nested child workspace.
fn handle_init(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let name = matches.get_one::<String>("name").unwrap();

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    init_child_workspace(name, &base_path)?;
    Ok(())
}

/// Handler for the check command: report or fix workspace drift.
fn handle_check(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let fix = matches.get_flag("fix");

    let base_path = if config.meta_root().is_some() {
        config.meta_root().unwrap()
    } else {
        config.working_dir.clone()
    };

    check_workspace(&base_path, fix)?;
    Ok(())
}

/// `meta repo label`: manage the labels that `--label` / `--exclude-label` select on.
fn label_command() -> clap::Command {
    let version = env!("CARGO_PKG_VERSION");
    let edit = |name: &'static str, about: &'static str| {
        clap::Command::new(name)
            .about(about)
            .version(version)
            .arg(
                clap::Arg::new("repo")
                    .required(true)
                    .help("Repo name, path, or alias"),
            )
            .arg(
                clap::Arg::new("labels")
                    .required(true)
                    .num_args(1..)
                    .help("One or more labels"),
            )
    };
    clap::Command::new("label")
        .about("Add, remove, or list repo labels")
        .version(version)
        .after_long_help(metarepo_core::format_help_description(
            "Labels are free-form words stored on a repo in the workspace config. Any\n\
             multi-repo command can then be narrowed with the global --label and\n\
             --exclude-label flags. Nested workspaces carry the built-in label @workspace.\n\
             \n\
             Examples:\n  \
               meta repo label add api backend rust       label a repo\n  \
               meta repo label list                       show every repo's labels\n  \
               meta --label backend git status            only repos labelled backend\n  \
               meta --label backend --label rust exec -- cargo test    both labels\n  \
               meta --label web,docs run build            either label\n  \
               meta --exclude-label legacy git pull       everything except legacy\n  \
               meta --label @workspace exec -- meta git status   only nested workspaces",
        ))
        .subcommand(edit("add", "Add labels to a repo"))
        .subcommand(edit("remove", "Remove labels from a repo").visible_alias("rm"))
        .subcommand(
            clap::Command::new("list")
                .visible_alias("ls")
                .about("List the labels of every repo in scope")
                .version(version),
        )
}

fn handle_label(matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
    let (action, sub) = match matches.subcommand() {
        Some(("list", _)) | None => {
            let root = config.meta_root();
            let keys = config.scoped_project_keys();
            let width = keys.iter().map(|k| k.len()).max().unwrap_or(0);
            for key in keys {
                let labels = config.meta_config.effective_labels(&key, root.as_deref());
                let shown = if labels.is_empty() {
                    "-".to_string()
                } else {
                    labels.join(", ")
                };
                println!("{key:<width$}  {shown}");
            }
            return Ok(());
        }
        Some(pair) => pair,
    };

    let path = config
        .meta_file_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No workspace config found. Run 'meta init' first."))?;
    // Reload from disk so only what is in the file gets written back.
    let mut meta = metarepo_core::MetaConfig::load_from_file(path)?;
    let repo = sub.get_one::<String>("repo").unwrap();
    let key = meta
        .resolve_identifier(repo)
        .filter(|k| meta.projects.contains_key(k))
        .ok_or_else(|| anyhow::anyhow!("Repo '{repo}' not found in the workspace config"))?;
    let wanted: Vec<String> = sub.get_many::<String>("labels").unwrap().cloned().collect();
    if let Some(bad) = wanted
        .iter()
        .find(|l| l.is_empty() || l.starts_with('@') || l.contains([',', ' ']))
    {
        anyhow::bail!("Invalid label '{bad}': labels cannot be empty, start with @ (reserved for built-ins), or contain commas or spaces");
    }

    // Labels live on the metadata form; upgrade a bare "name": "url" entry.
    if let Some(metarepo_core::ProjectEntry::Url(url)) = meta.projects.get(&key) {
        let upgraded = serde_json::from_value(serde_json::json!({ "url": url }))?;
        meta.projects
            .insert(key.clone(), metarepo_core::ProjectEntry::Metadata(upgraded));
    }
    let Some(metarepo_core::ProjectEntry::Metadata(metadata)) = meta.projects.get_mut(&key) else {
        unreachable!("entry was just upgraded to the metadata form");
    };
    if action == "add" {
        metadata.labels.extend(wanted);
        metadata.labels.sort();
        metadata.labels.dedup();
    } else {
        metadata.labels.retain(|l| !wanted.contains(l));
    }
    let now = if metadata.labels.is_empty() {
        "(none)".to_string()
    } else {
        metadata.labels.join(", ")
    };
    meta.save_to_file(path)?;
    println!("{key}: {now}");
    Ok(())
}

// Traditional implementation for backward compatibility
impl MetaPlugin for ProjectPlugin {
    fn name(&self) -> &str {
        "repo"
    }

    fn register_commands(&self, app: clap::Command) -> clap::Command {
        // Delegate to the builder-based plugin
        let plugin = Self::create_plugin();
        // `project` was the original name; keep it working as an alias.
        plugin.register_commands(app).mut_subcommand("repo", |c| {
            c.visible_alias("project").subcommand(label_command())
        })
    }

    fn handle_command(&self, matches: &ArgMatches, config: &RuntimeConfig) -> Result<()> {
        if let Some(("label", label_matches)) = matches.subcommand() {
            return handle_label(label_matches, config);
        }
        // Delegate to the builder-based plugin
        let plugin = Self::create_plugin();
        plugin.handle_command(matches, config)
    }
}

impl BasePlugin for ProjectPlugin {
    fn version(&self) -> Option<&str> {
        Some(env!("CARGO_PKG_VERSION"))
    }

    fn description(&self) -> Option<&str> {
        Some("Project management operations")
    }

    fn author(&self) -> Option<&str> {
        Some("Metarepo Contributors")
    }
}

impl Default for ProjectPlugin {
    fn default() -> Self {
        Self::new()
    }
}
