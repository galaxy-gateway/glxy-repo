# meta project add: confirmation and interrupt safety

Tracked in [#179](https://github.com/codyaverett/metarepo/issues/179).

## Confirmation

In an interactive terminal, `meta project add` prints the workspace root, the
resolved project path, the source, and the layout (bare/regular, depth,
recursive) and asks "Proceed?" before touching disk. Pass `--yes` / `-y` to
skip it. Non-interactive runs (pipes, CI) never prompt.

## Ctrl+C and failures during clone

- The project directory is only removed if this run created it (it did not
  exist beforehand). Pre-existing directories are never deleted.
- Ctrl+C sets a cancel flag checked by the libgit2 transfer progress
  callback; the clone aborts, the guard removes the partial directory, and the
  command exits with "Clone cancelled by user". `.meta` and `.gitignore` are
  only written after a successful clone, so they are untouched.
- The same cleanup runs if a later bare-layout step (default worktree) fails.
- Outside the clone, Ctrl+C keeps the default behaviour (exit 130).

## Leftover directories

If a project directory exists but is not a valid git repository (for example
from an older interrupted clone) and a URL source is given, `add` now errors
instead of registering it. Remove the directory and retry.

## Known gaps

Recursive imports of nested projects use the same guard per project, but an
interrupt does not roll back projects already imported earlier in the run.
