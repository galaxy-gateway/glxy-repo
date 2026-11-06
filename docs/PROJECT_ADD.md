# meta project add: confirmation and interrupt safety

Tracked in [#179](https://github.com/codyaverett/metarepo/issues/179),
[#180](https://github.com/codyaverett/metarepo/issues/180),
[#181](https://github.com/codyaverett/metarepo/issues/181) and
[#182](https://github.com/codyaverett/metarepo/issues/182).

## Option review

In an interactive terminal, `meta project add` asks for the layout (bare +
worktree or regular clone) and history (full or shallow with a depth), unless
`--bare` / `--depth` were given. It then shows the workspace root, resolved
path, source and layout and offers Proceed / Edit options / Cancel. Edit
re-asks everything, including the project path, so the location can be changed
before anything touches disk. Pass `--yes` / `-y` to skip the review.
Non-interactive runs (pipes, CI) never prompt.

## Progress and timing

Clone and default-worktree steps show a spinner with live object and byte
counts and elapsed time (plain lines when stdout is not a TTY). Each step
prints a done line with its duration, and the final line shows the total, for
example `Successfully added 'x' in 12.4s`. `meta project update` and the git
sync commands already print per-project durations in their summary tables.

## Change report and commit offer

After `project add` and `project remove`, the command lists what changed:
created or modified `.meta` and `.gitignore` (with added and removed lines)
and the project directory created or deleted. In a git workspace it then
offers to commit just those files as `chore(meta): add project X` (or
`remove project X`). A pathspec-limited commit is used so other staged work is
never swept in. `--commit` commits without asking, `--no-commit` skips the
offer; non-interactive runs without a flag print the manual command instead.
A failed commit (hook, signing) warns and does not fail the command.

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
