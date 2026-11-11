# Repo labels

Labels are free-form words stored on a repo in the workspace config. Any
multi-repo command can then be narrowed to the repos that carry a label, without
listing names and wherever you are in the workspace.

## Managing labels

```bash
meta repo label add api backend rust     # add one or more labels
meta repo label remove api rust          # remove (alias: rm)
meta repo label list                     # every repo in scope with its labels
```

Labels live on the metadata form of a project entry. `label add` upgrades a
short `"name": "url"` entry to that form for you:

```json
{
  "projects": {
    "api": {
      "url": "git@github.com:acme/api.git",
      "labels": ["backend", "rust"]
    }
  }
}
```

A label cannot be empty, contain commas or spaces, or start with `@` (reserved
for built-in labels).

## Selecting by label

Two global flags work with every multi-repo command (`git`, `exec`, `run`,
`worktree`, `repo list`, `status`, `workspace list`). They can go before or
after the subcommand.

| Flag | Meaning |
|------|---------|
| `--label a` | only repos labelled `a` |
| `--label a,b` | repos labelled `a` **or** `b` |
| `--label a --label b` | repos labelled `a` **and** `b` |
| `--exclude-label a` | every repo except those labelled `a` (comma-separated, repeatable) |

```bash
meta --label backend git status
meta --label backend --label rust exec -- cargo test
meta --label web,docs run build
meta --exclude-label legacy git pull
meta --label backend wt add feature/login
meta --label backend --exclude-label legacy exec --all --parallel -- make lint
```

Labels **narrow** whatever scope is already selected; they never widen it:

- The directory-aware scope still applies. Inside `services/`, `--label backend`
  selects the backend repos beneath `services/`. Add `-w` for the whole workspace.
- With `--all`, `--project` or `--projects`, repos that do not match are skipped
  (a named repo that is filtered out prints a note saying so).
- Disabled repos stay excluded. `--include-disabled` brings back disabled repos
  but never overrides a label selection.

## Built-in labels

Built-in labels start with `@` and are assigned by meta, not stored in the config.

| Label | Carried by |
|-------|------------|
| `@workspace` | a repo that is itself a meta workspace (a nested workspace) |

```bash
meta --label @workspace repo list             # the nested workspaces
meta --label @workspace exec -- meta git status   # run meta inside each one
meta --exclude-label @workspace git pull      # only the plain repos
```

A nested `meta` started by `meta exec` does not inherit the outer selection, so
the second example shows every repo inside each nested workspace.

## Notes and limits

- Exclusion is spelled `--exclude-label` rather than a `~` or `!` prefix because
  those characters are expanded by zsh and bash.
- External plugins receive the selection over the plugin protocol
  (`label_include` / `label_exclude`, see
  [PLUGIN_PROTOCOL_V1.md](PLUGIN_PROTOCOL_V1.md)), but each command has to use
  it. Today `meta rules check` honors it; the other `rules` commands and the
  `mcp` plugin do not.
- Labels select repos for commands. They do not create clone profiles:
  `meta git update` clones every enabled repo. See
  [DISABLING_PROJECTS.md](DISABLING_PROJECTS.md) to keep repos out of a workspace.
