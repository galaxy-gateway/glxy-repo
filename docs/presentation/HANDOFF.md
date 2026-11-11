# Presentation handoff

Handoff for an agent improving the metarepo (`meta`) slide deck. Read this whole file first.

## Goal and audience

A visual presentation about `meta` for **developers who use git and run AI agents (Claude Code and similar) from the CLI**. It should cover: what the project is, diagrams, examples, use cases, patterns, **when not to use it**, opt-in composition patterns, sharing tooling across repos, and extending meta with plugins and modules (the user calls these "mods").

Tone: technical, honest about limits, no marketing fluff.

## Current state (17 slides, commit ae24365)

| # | id | Content |
|---|----|---------|
| 1 | cover | Title, animated repo grid |
| 2 | problem | Drifting repos, manual cross-repo loop |
| 3 | manifest | `.meta` file plus animated CLI, plugins, repos diagram |
| 4 | fleet-git | `meta git` commands and the results table |
| 5 | fan-out | `exec`, `run`, directory-aware scope |
| 6 | worktrees | Bare-first layout, `worktree add --all` |
| 7 | agents | Worktree per agent (teal statement slide, animated) |
| 8 | harness | Four ways to make an agent fluent in meta |
| 9 | use-cases | Four use cases |
| 10 | extend | Manifest plugin, protocol plugin, module |
| 11 | hub | Pattern 1: hub workspace (animated diagram) |
| 12 | nested | Pattern 2: nested workspaces (animated diagram) |
| 13 | shared-tooling | Patterns 3 and 4: tooling repo, modules |
| 14 | opt-in | What is opt-in (table) |
| 15 | not-for | When not to use it (light slide) |
| 16 | not-yet | Not well suited yet (light slide) |
| 17 | start | Five commands to start |

Every slide has speaker notes in an `<aside>`.

## Files and the two renderers

Source of truth is this folder. Edit `slides/<id>.html` and `deck.json` (index: `order`, `sections`, `faces`).

- **Local viewer:** `python3 docs/presentation/build.py` writes `index.html`; open it in a browser. `build.py` supplies default CSS, turns `<x-connector>` into SVG arrows, and fades in `data-build-in` elements. Keys: arrows, space, Home, End, `n` notes, `f` fullscreen.
- **claude.ai Slides artifact:** https://claude.ai/artifact/ERpQGeYPdoHxamawL7BH1k (private to the user). It is the published copy; it can drift from this folder. Republish with the Artifact tool using `url` above, `root` = a folder containing `project/deck.json` and `project/slides/*.html`, and `file_path` = `project/deck.json`. The earlier scratchpad copy was temporary: recreate the `project/` layout from this folder (`deck.json` to `project/deck.json`, `slides/` to `project/slides/`). Do not publish `index.html`.

## Slide format rules (the claude.ai subset; keep slides compatible with both)

- One `<section id="<id>">` per file, fixed **1920x1080** canvas, all styles inline.
- Allowed: `px` lengths, hex colors, flex or grid layout, `position:absolute` pins, `<h1-3>`, `<p>`, `<div>`, `<table>`, `<b>`, `<span style="color">`, `<x-connector>`, `<x-shape>`, `<x-icon>`, `<svg aria-label>`, `data-build-in="fade|rise|pop N"` (on pinned children only), `data-transition`.
- Not allowed: classes, `<style>`, `margin`, `z-index`, `em`/`var()`, nested lists, `<script>`, external images or `data:` URIs.
- Text no smaller than 24px; 128px slide padding; keep content above y=952.
- Palette: bg `#0f1419`, cards `#182028`, borders `#2a3540`, text `#e8ebe6`, muted `#9aa7b2`, accents teal `#5ad1b8` and orange `#f0a35e`. Light slides: `#e9ebe4`. Fonts: Space Grotesk (headings), Inter (text), JetBrains Mono (code).
- Full spec lives in the artifact type's `artifact-type/reference/format.md`; read it via the Artifact `read` action on the artifact URL if you need exact rules.

## How to verify (do not use keystroke injection or screen capture)

The Playwright MCP browser is not installed here. Use the cached headless Chromium directly:

```bash
H=$(find ~/Library/Caches/ms-playwright/chromium_headless_shell-1208 -name chrome-headless-shell -type f | head -1)
# URL fragments are ignored by --screenshot, so force the slide number in a temp copy:
sed "s#parseInt(location.hash.slice(1))||1#N#" docs/presentation/index.html > /tmp/tN.html
"$H" --no-sandbox --disable-gpu --window-size=1920,1080 --virtual-time-budget=6000 \
  --screenshot=/tmp/sN.png "file:///tmp/tN.html"
```

Then Read the PNG. For overflow checks, measure each section's bottom-most element in a stacked preview (hide `aside` and `x-connector`) and flag anything past 952px or any element where `scrollWidth > clientWidth`.

Gotchas already hit:
- Inline `display:flex` on a section beats stylesheet `display:none`; `build.py` hides inactive slides with `section:not(.show){display:none!important}`.
- Headless virtual time does not flush `requestAnimationFrame`; `build.py` forces a synchronous reflow instead.
- Only the original 15 slides had a full overflow pass (tightest: manifest at 915px). `extend` and `not-yet` were not overflow-checked. Only slides 3, 11 and 16 have been viewed in the local viewer.

## Verified vs unverified claims

Sources used: `README.md`, `docs/PRODUCT.md`, `docs/WORKTREE.md`, `docs/HARNESS_INTEGRATION.md`, `docs/MODULES.md`, `docs/PLUGIN_DEVELOPMENT.md`, `docs/AGENT_HOOKS.md`.

Verify before presenting as fact:
- **"No tags or profiles for partial cloning"** (not-yet slide): based only on a doc grep. Check the code and `docs/DISABLING_PROJECTS.md`.
- **Shared tooling pattern** (shared-tooling slide): inferred from this repo listing `.claude`, `.github`, `scripts` as projects in `.meta`. `meta run`, `meta skill install` and `meta module enable` are real commands, but confirm the exact flags and behavior with `meta <cmd> --help`.
- **`.meta` snippet, agent session names, and repo names (api, web, acme)** are illustrative, not captured output. Prefer real captured output where possible.
- **`meta --root -w git status`** (nested slide): flags come from the README scope section; run it against a real nested workspace to confirm.
- Module gaps (no scaffolder, registry, module-level pin) come from the "Out of scope" list in `docs/MODULES.md`, which may be stale; check the current CLI.

## Improvement backlog (suggested, in priority order)

1. Run the full overflow and visual pass on all 17 slides in the local viewer; fix anything off.
2. Replace illustrative snippets with real captured terminal output (a real `meta git status` table, a real `worktree add --all` run).
3. Add a nested-workspace and a worktree fan-out diagram that shows two repos changing together (one branch, N repos).
4. Add a short "decision" slide: a flowchart of "should I use meta?" tying together use cases and limits.
5. Add a slide on the plugin protocol (one JSON message exchange) for the extend section, using `docs/PLUGIN_PROTOCOL_V1.md`.
6. Consider a version badge pulled from `Cargo.toml` at build time instead of hardcoded "v0.83.0" on the cover.
7. Add a print/PDF stylesheet or export script to `build.py` if the user wants a PDF.
8. Light-slide polish: `not-yet` is half empty; add a third item only if it is verified.

## Rules for the working agent

From the project and user CLAUDE.md files:
- Stage and commit finished work automatically; use GPG-signed commits (`git commit -S`), then verify with `git log --show-signature -1`. Do not add `timeout` (not installed on this Mac).
- Conventional Commits format, shell-safe (no backticks, exclamation marks, angle brackets, em-dashes or double quotes in the message). Include the attribution lines the session asks for, none otherwise.
- If a package version changes, bump it in the commit; docs-only changes do not need a bump. Never push tags without user approval.
- Never invent statistics or quotes; label anything illustrative.
- Do not create or push to external services without the user's say-so. Republishing the artifact is allowed (it stays private); sharing is the user's call.
- When mentioning GitHub issues or PRs, write them as embedded links built from the origin remote.
- Document plans and findings under `docs/` (plan docs go in `docs/plans/`, matching the existing format).
