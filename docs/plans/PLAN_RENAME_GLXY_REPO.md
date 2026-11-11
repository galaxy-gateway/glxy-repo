# Plan: Rename metarepo to glxy-repo

Status: proposed (decisions below need the owner's sign-off before work starts)
Related issues: #186 (binary name collision and searchability)
Date: 2026-11-11

## Goal

Publish the tool as `glxy-repo`, matching the `glxy-` convention of the other
galaxy-gateway projects (`glxy-claw`, `glxy-gha`, `glxy-passman`, `glxy-term`),
so it is searchable, no longer collides with the Node.js `meta` tool, and has
one name for crate and command. Existing users and workspaces keep working.

## Current state

- Crate `metarepo` (0.84.0 on crates.io), binary `meta` (`meta/Cargo.toml`).
- Sibling crates: `metarepo-core`, `metarepo-plugin-sdk`, and the external
  plugins under `plugins/`.
- Manifest file `.metarepo` (legacy `.meta`); GitHub repo
  `galaxy-gateway/glxy-repo` (was `codyaverett/metarepo`).
- `glxy-repo` and `glxy` were unclaimed on crates.io on 2026-11-10.

## Decisions

Recommended defaults; each is the owner's call.

1. **Crate and command: `glxy-repo`.** Decided.
2. **Keep `meta` as a second binary for a deprecation window.** Same program,
   so scripts and muscle memory survive. Remove it at 1.0 or after two minor
   releases, whichever is later.
3. **Do not ship a bare `glxy` binary.** `glxy` is the brand prefix shared by
   several tools; one of them owning the bare command blocks a future umbrella
   CLI. Users who want it add a shell alias; the README shows the line.
   Open: the owner suggested `glxy` as an alias, so confirm.
4. **Leave the library and plugin crate names alone for now**
   (`metarepo-core`, `metarepo-plugin-sdk`, `metarepo-plugin-*`). Plugin specs
   and lockfiles pin them by name; renaming them is a separate, breaking step.
   If they are ever renamed, they take the full prefix (`glxy-repo-core`,
   `glxy-repo-plugin-sdk`), never `glxy-core`, which would collide with the
   other glxy projects. Open.
5. **Leave the manifest names alone** (`.metarepo`, `.meta`) and any config
   paths. No workspace migration.
6. **GitHub repo:** done 2026-11-11. Transferred from `codyaverett/metarepo`
   to `galaxy-gateway/glxy-repo`; the old URL redirects.

## Approach

`cargo install glxy-repo` installs `glxy-repo` and `meta` from one crate (two
`[[bin]]` targets; a three-line `src/bin/meta.rs` that calls the shared entry
point avoids the shared-path warning). Help and error text take the command
name from `argv[0]` so each binary describes itself. crates.io cannot rename a
crate, so `metarepo` gets one last release whose README and description point
at `glxy-repo`.

## Phases

### Phase 1: Reserve and dual-ship
1. Owner publishes `glxy-repo` (and optionally a placeholder `glxy`) to hold
   the names.
2. Rename the package in `meta/Cargo.toml`, add the second binary, make help
   text follow `argv[0]`, update release workflows and archive names.

### Phase 2: Docs and surface
1. Sweep README, `docs/`, the slide deck, the bundled skill and help text from
   `meta ...` to `glxy-repo ...`, with one note that `meta` still works.
2. Rename the GitHub repo and update `repository`/`homepage` and badges.

### Phase 3: Retire the old names
1. Final `metarepo` release pointing at `glxy-repo`.
2. `meta` prints a one-line deprecation notice on stderr; later, remove it.

## Risks

- Download history and search ranking for `metarepo` reset to zero.
- `glxy-repo` is nine characters with a hyphen; daily use depends on users
  aliasing it, so the README must make that a one-liner.
- Tests invoke `CARGO_BIN_EXE_meta`; they keep passing while `meta` ships, and
  must move before it is removed.

## Definition of done

`cargo install glxy-repo` gives a working `glxy-repo` and `meta`; all four
Rust checks pass; README and docs lead with the new name; the last `metarepo`
release points to it.

## Tasks

1. Reserve glxy-repo on crates.io
   Owner publishes a first glxy-repo release (and decides on a glxy placeholder).
   priority: high
2. Rename the package to glxy-repo and ship glxy-repo plus meta binaries
   Rename in meta/Cargo.toml, add src/bin/meta.rs, make help and error text use
   argv[0], update the Makefile bump target, release-binaries workflow and
   archive names. One test asserts both binaries run.
   blocked by: 1
   priority: high
3. Sweep docs, deck, skill and help text to the glxy-repo name
   README, docs/, docs/presentation, the bundled meta-tool skill. State once
   that meta remains as a deprecated alias and show the shell alias line.
   blocked by: 2
   priority: medium
4. Rename the GitHub repository and update links and badges
   blocked by: 2
   priority: medium
5. Publish a final metarepo release that points to glxy-repo
   blocked by: 2
   priority: medium
6. Deprecate then remove the meta binary
   Stderr notice first; removal at 1.0 or two minors later. Move tests off
   CARGO_BIN_EXE_meta before removal.
   blocked by: 3
   priority: low
