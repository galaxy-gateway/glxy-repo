# Stability and versioning

Status: **draft for owner review**. Nothing here is a commitment until this line is removed.

`meta` is pre-1.0. This page says what you can depend on between releases, what you cannot, and how you find out when something breaks.

## Stable

A breaking change to anything in this list is made only in a minor release and is called out in the changelog.

- **The manifest.** The filenames in [CONFIG_FORMATS.md](./CONFIG_FORMATS.md) (`.metarepo`, its `.json`, `.yaml`, `.yml` and `.toml` variants, and the legacy `.meta`) keep being discovered. A manifest that loads today keeps loading; new fields are optional.
- **The core multi-repo commands**, Surface A in [PRODUCT.md](./PRODUCT.md): `meta init`, `repo`, `git`, `exec`, `run`, `worktree`, `config`, `status`, and the scoping flags `-w`, `--root`, `-c`/`--config`. Their names, subcommands and documented flags stay. When one is renamed, the old name stays as an alias, as `meta project` did when it became `meta repo` in 0.84.0.
- **Plugin protocol v1.** [PLUGIN_PROTOCOL_V1.md](./PLUGIN_PROTOCOL_V1.md) has been stable since 0.19.0. Minor protocol revisions only add optional fields, so a plugin built against an older `1.x` keeps loading. A breaking change means protocol `2.0`.

## Not stable

- **Anything behind `-x` / `--experimental`.** It may change or be removed in any release.
- **Output meant for people**: tables, colors, spinners, prompts, wording of messages and help text, and the `status` and `worktree tui` screens. Do not parse it.
- **The agent and extension commands** (`meta skill`, `module`, `plugin`), the external plugins (`mcp`, `rules`), and newer commands such as `meta workspace` and labels. PRODUCT.md marks some of these stable in the sense of not experimental; they are not yet covered by the promise above.
- **Rust library APIs** of `metarepo-core` and `metarepo-plugin-sdk`. Follow each crate's own version.
- Anything not listed under Stable.

## What a release number means while 0.x

- **Patch** (`0.84.0` to `0.84.1`): bug fixes only. No intentional breaking change to a stable surface.
- **Minor** (`0.84` to `0.85`): new features, and possibly breaking changes. Read the changelog before upgrading across a minor.
- A fix for a security problem or data loss may break behavior in any release.

Pin an exact version (`cargo install metarepo --version X.Y.Z`) wherever an unplanned change would hurt, such as CI.

## How breaking changes are announced

In [meta/CHANGELOG.md](../meta/CHANGELOG.md), in the entry for the release that makes the change, under Changed or Removed, with the word **Breaking** and the migration step. The changelog is the only channel; there is no mailing list.

A rename of the tool to `glxy-repo` is planned, with `meta` kept as an alias for a transition window; see [the rename plan](./plans/PLAN_RENAME_GLXY_REPO.md).

## Proposed criteria for 1.0

1.0 means breaking changes to the stable surfaces above need a major version. Proposed conditions, none dated:

- The rename has landed and the fate of the `meta` alias is decided.
- Every manifest field is documented in one place.
- The Not stable list has been reviewed item by item: each entry is promoted, or stays excluded on purpose.
- There is a decision on machine-readable output and exit codes for scripts.
- Several consecutive minor releases have shipped with no breaking change to a stable surface.
