# meta

**One workspace for all your git repos.** A fast Rust CLI that lets you treat a folder full of independent repositories as a single thing, without merging them into a monorepo.

[![crates.io](https://img.shields.io/crates/v/metarepo.svg)](https://crates.io/crates/metarepo)
[![CI](https://github.com/galaxy-gateway/glxy-repo/actions/workflows/ci.yml/badge.svg)](https://github.com/galaxy-gateway/glxy-repo/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```bash
meta git status                      # every repo, one answer
meta exec --parallel -- npm test     # one command, every repo
meta wt add feature/login --all      # one branch, a folder in every repo
```

If you have ever typed `for d in */; do (cd "$d" && git pull); done` and felt a little bit of your soul leave your body, this is for you.

## Why

You have more than a couple of repos. They were supposed to stay in sync. They did not.

- One is three branches behind. One has uncommitted changes from a Tuesday you no longer remember.
- A change that touches four of them means four terminals, four branches and four chances to forget one.
- Your shell history is a museum of `cd ../other-thing`.

`meta` adds one small file next to your repos that lists them. After that, git commands, shell commands, named scripts and branches fan out across all of them, or just the ones you are standing in, or just the ones you labelled `backend`. Your repos stay exactly as they are: own history, own remotes, own CI. Delete the file and nothing else changes.

## Is this for you?

**Probably yes if**

- you work across three or more repos (microservices, app plus API plus infra, a pile of libraries)
- you want one status view and one command instead of a loop
- you want a feature branch to exist in several repos at once, without stashing
- you run AI coding agents and want each one in its own isolated checkout

**Probably not if**

- you have one or two repos. Plain git is great. Go outside.
- you need atomic commits across projects, or build caching and affected-only tests. That is a monorepo with Nx, Turborepo or Bazel.
- nobody on the team opens a terminal.

`meta` coordinates repos. It does not understand your build graph, and it does not open pull requests for you.

## Install

```bash
cargo install metarepo        # the crate is metarepo, the command is meta
meta --version
```

No Rust toolchain? Grab a prebuilt binary for macOS, Linux or Windows from the [releases page](https://github.com/galaxy-gateway/glxy-repo/releases), unpack it, and put `meta` on your `PATH`.

From a clone of this repo: `cargo install --path meta`.

## Five-minute tour

```bash
mkdir acme && cd acme
meta init                                         # writes .metarepo, the list of your repos

meta repo add api git@github.com:acme/api.git     # clone and track
meta repo add web git@github.com:acme/web.git
meta repo list                                    # the workspace as a tree

meta git status                                   # where does everything stand?
meta git pull                                     # update the lot
meta exec --parallel -- npm test                  # any command, every repo, at once

meta wt add feature/login --all                   # the same branch, a folder in every repo
```

Every multi-repo run ends with one results table, so you never scroll back to find out which repo was the problem:

```
  Project    Status   Time  Detail
  ────────────────────────────────
  api        ok       0.1s  2 files changed, 2 insertions(+)
  web        skipped        uncommitted changes
  docs       skipped        no upstream tracking branch
  1 ok, 2 skipped
```

Joining a workspace someone else set up is two commands:

```bash
meta git clone https://github.com/acme/workspace.git
cd workspace && meta git update     # clones every repo listed in the manifest
```

Want the long version with pictures? Clone this repo and run `make slides` for a 23-slide animated walkthrough.

> **Heads-up: repos are added "bare-first" by default.** `meta repo add api ...` creates `api/main/` (your default branch) so that other branches can live beside it as `api/feature-login/`. It is lovely once you know, and surprising if you do not. Prefer ordinary clones? Answer "regular" when `repo add` asks, or set `"default_bare": false` in the manifest. See [Worktrees](#worktrees-and-the-bare-first-layout).

## What you get

| | |
|---|---|
| **Fleet git** | `meta git status`, `pull`, `push`, `fetch`, `checkout`, `sync`, `clone` across every repo, skipping the ones that are not ready and telling you why |
| **Run anything** | `meta exec -- <command>` in each repo, in parallel if you like |
| **Named scripts** | `meta run build`: define a script once in the manifest, run it everywhere |
| **Directory-aware scope** | commands act on the repo you are in, the folder you are in, or everything at the root |
| **Labels** | `meta --label backend git pull`: tag repos once, target them forever |
| **Worktrees** | `meta wt add feature/x --all`: parallel branches as folders, no stashing |
| **Nested workspaces** | `meta ws init teams/payments`: a team-sized workspace inside an org-sized one |
| **Status dashboard** | `meta status`: a live, read-only terminal dashboard |
| **Plugins** | add your own subcommands in any language |
| **Agent support (opt-in)** | a Claude Code skill, an MCP server, per-agent worktrees |

### How it compares

| Instead of | You get | You give up |
|---|---|---|
| a shell loop over folders | scope, parallelism, a results table, skipped-with-a-reason | the quiet pride of a hand-rolled `for` loop |
| git submodules | repos that stay independent, no pinned SHAs, no detached HEAD surprises | exact cross-repo version pinning |
| a monorepo | no migration, per-repo history, access control and CI | atomic commits, one build graph |
| the Node.js [meta](https://github.com/mateodelnorte/meta) tool (the inspiration) | a single binary with no Node runtime, the same `.meta` file format, plus worktrees, labels and plugins | the npm plugin ecosystem |

## Everyday use

### Repos and projects are the same thing

`meta repo` and `meta project` are the same command. A repo is a project and a project is a repo. Use whichever word your brain reaches for.

```bash
meta repo add api git@github.com:acme/api.git
meta repo add api git@github.com:acme/api.git --depth 1   # shallow clone
meta repo list
meta repo remove api
meta repo check                                           # report workspace drift
```

### Scope follows where you stand

Multi-repo commands work on a set of repos decided by your current directory:

- **inside a repo**: just that repo
- **inside a folder of repos**: the repos beneath it
- **at the workspace root**: all of them

```bash
cd services/
meta git status        # only the repos under services/
meta -w git status     # every repo, from anywhere
```

`--project` / `--projects` pick repos by name. `--root` drives the outermost workspace when workspaces are nested.

### Labels

Label repos once, then narrow any command to them. Full guide: [docs/LABELS.md](docs/LABELS.md).

```bash
meta repo label add api backend rust
meta repo label list

meta --label backend git status                        # only backend repos
meta --label backend --label rust exec -- cargo test   # both labels
meta --label web,docs run build                        # either label
meta --exclude-label legacy git pull                   # everything except legacy
meta --label @workspace exec -- meta git status        # only nested workspaces
```

### Scripts

Scripts live in the manifest, so the whole team shares them.

```json
{ "scripts": { "test": "npm test", "build": "npm run build" } }
```

```bash
meta run --list
meta run test
meta run build --parallel
meta run --tui          # pick a script and watch per-repo output live
```

### Worktrees and the bare-first layout

A worktree is a second checkout of the same repo in its own folder, on another branch. No stashing, no "wait, which branch am I on".

```bash
meta wt add feature/login --all      # create the branch and folder in every repo
meta wt list
meta wt remove feature/login --all
meta wt clean                        # remove worktrees whose branches are merged
```

By default a repo added through `meta` is laid out like this:

```
workspace/
└── api/
    ├── .git/            bare repository
    ├── main/            default branch
    └── feature-login/   another branch, side by side
```

Set `"default_bare": false` in the manifest (or `"bare": false` per repo) for traditional clones. A `worktree_init` hook can run your install step whenever a worktree is created:

```json
{
  "worktree_init": "npm ci",
  "projects": {
    "api": { "url": "git@github.com:acme/api.git", "worktree_init": "cargo build" }
  }
}
```

Pass `--no-hooks` to skip it. Details: [docs/WORKTREE.md](docs/WORKTREE.md).

### Nested workspaces

```bash
meta ws init teams/payments     # create a workspace inside this one and register it
meta ws list
meta --root -w git status       # from inside a nested workspace, reach everything
```

### Shallow clones and whole-org sweeps

`--depth N` on `meta repo add` or `meta git clone` is remembered per repo, so later re-clones stay shallow. Handy for pulling an entire GitHub org down for an audit:

```bash
meta init
gh repo list ORG --limit 1000 --json name,url --jq '.[] | "\(.name) \(.url)"' \
  | while read -r name url; do meta repo add "$name" "$url" --depth 1; done
```

`meta git pull --shallow` re-truncates history after pulling.

### The manifest

`meta init` writes `.metarepo` (JSON). Prefer something else? `meta init --format yaml` or `--format toml`, and `meta config migrate yaml` converts later. Workspaces created by the Node.js tool use `.meta`; that name is read too, so they work unchanged. The repos in your manifest are the only thing `meta` knows about. No hidden state, no database, no daemon.

## Command reference

| Command | What it does |
|---|---|
| `meta init [--with-skill\|--with-completions\|--all]` | create or repair a workspace; optional shell completions and agent skill |
| `meta repo <add\|list\|remove\|rename\|label\|check\|update\|convert-to-bare>` | manage the repos in the workspace (alias: `project`) |
| `meta git <clone\|status\|update\|sync\|pull\|push\|fetch\|checkout\|branch>` | git across the fleet |
| `meta exec [flags] -- <command>` | run a shell command in each repo |
| `meta run [flags] <script>` | run a script from the manifest |
| `meta worktree <add\|list\|remove\|clean\|prune\|repair\|tui>` | worktrees across repos (alias: `wt`) |
| `meta workspace <init [name]\|list>` | workspaces and nested workspaces (alias: `ws`) |
| `meta status` | interactive status dashboard |
| `meta config <get\|set\|list\|edit\|migrate>` | read and change configuration |
| `meta plugin <install\|list\|verify\|update\|remove>` | manage external plugins |
| `meta skill`, `meta module` | agent skills and plugin-plus-skill bundles (opt-in) |

Global flags:

| Flag | Meaning |
|---|---|
| `-w`, `--workspace` | act on every repo, ignoring the current directory |
| `--root` | use the outermost enclosing workspace |
| `--label`, `--exclude-label` | narrow to, or skip, repos by label |
| `-c`, `--config <path>` | use a specific config file |
| `--non-interactive <fail\|defaults>` | never prompt (for CI) |
| `-x`, `--experimental` | enable experimental features |

`meta <command> --help` has the full story for each one, with examples.

## For people who work with AI agents

Entirely optional. Nothing here is needed to use the commands above.

- `meta init --with-skill` installs a Claude Code skill that teaches an agent the CLI.
- One worktree per agent task (`meta wt add fix/cache --all`) means parallel sessions never trip over each other.
- The `mcp` plugin exposes `meta` to any MCP-capable harness.

See [docs/HARNESS_INTEGRATION.md](docs/HARNESS_INTEGRATION.md) and [docs/PRODUCT.md](docs/PRODUCT.md).

## Extending

Add your own subcommands without touching this codebase:

- **Manifest plugin**: any executable plus a small manifest. Shell, Python, Go, whatever you have.
- **Protocol plugin**: a subprocess speaking JSON over stdio, with structured access to the workspace. There is a Rust SDK.
- **Module**: a repo that bundles a plugin with the agent skills that drive it.

Start with [docs/PLUGIN_DEVELOPMENT.md](docs/PLUGIN_DEVELOPMENT.md) and the templates in [examples/](examples/).

## Documentation

- [CLI reference](.claude/skills/meta-tool/SKILL.md): every command and flag
- [Labels](docs/LABELS.md) · [Worktrees](docs/WORKTREE.md) · [Disabling repos](docs/DISABLING_PROJECTS.md) · [Config formats](docs/CONFIG_FORMATS.md) · [Shell completions](docs/SHELL_COMPLETIONS.md)
- [Plugin development](docs/PLUGIN_DEVELOPMENT.md) · [Plugin protocol](docs/PLUGIN_PROTOCOL_V1.md) · [Modules](docs/MODULES.md) · [Rules](docs/RULES.md)
- [Harness integration](docs/HARNESS_INTEGRATION.md) · [MCP gateway](docs/MCP_GATEWAY.md)
- [Architecture](docs/ARCHITECTURE.md) · [Product boundaries](docs/PRODUCT.md) · [Roadmap ideas](docs/TODO.md)

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

```bash
git clone https://github.com/galaxy-gateway/glxy-repo.git && cd glxy-repo
make install-hooks      # format, clippy and friends on every commit
cargo test
```

- Report a bug or pitch an idea: [new issue](https://github.com/galaxy-gateway/glxy-repo/issues/new/choose), or `make issue-bug` / `make issue-feature` / `make issue-idea` from a clone.
- Commits follow Conventional Commits.
- Found a security problem? Please follow the [security policy](SECURITY.md) rather than opening a public issue.

This repo is itself a meta workspace (see [`.meta`](.meta)), because it would be awkward otherwise.

## License

[MIT](LICENSE)
