# Presentation handoff

Handoff for an agent improving the metarepo (`meta`) slide deck. Read this whole file first.

## Goal and audience

A visual presentation that convinces **developers and product managers** to use `meta` and teaches them how. Developers use git and run AI agents (Claude Code and similar) from the CLI; product managers need plain language. It covers: what the project is, the five everyday moves with animated terminal examples, agents, use cases, patterns, **when not to use it**, opt-in composition, sharing tooling, and extending meta with plugins and modules ("mods").

Tone: warm and a little playful, but technical and honest about limits. No marketing fluff, no invented numbers.

## Design system (redesign of 2026-11-10)

- **The cast.** Six repos (`api`, `web`, `docs`, `infra`, `sdk`, `worker`) are drawn as small coloured characters with blinking eyes (inline SVG, ids `c-<name>`, labels `l-<name>`). They carry the story: tidy on the cover, scattered and worried on `problem`, lined up under the manifest on `idea`, fanned out, scoped, and branched later. Every repo is always labelled by name, so colour is never the only signal.
- **Magic move.** Slides with `data-transition="magic"` glide the cast to its place on the next slide because the ids match. Currently: cover, problem, fan-out, scope.
- **Live terminals.** `setup`, `status`, `fan-out`, `worktrees`, `labels` and `start` hold an `<x-embed>`: a tiny self-contained page that types the command and prints output on a loop. It respects `prefers-reduced-motion` (prints the final state, no typing). Text inside is 26px.
- **Accessibility.** Plain-language glossary slide (`words`), one idea per slide, 24px minimum text, high-contrast palette, statuses stated in words, reduced-motion support in the viewer and the embeds, speaker notes on every slide.

## Current state (23 slides)

| # | id | Content |
|---|----|---------|
| 1 | cover | Title and the six-repo cast |
| 2 | problem | The cast scattered, each with a kind of drift |
| 3 | idea | `.metarepo` manifest, cast lined up under it |
| 4 | words | Five words to know (light slide) |
| 5 | setup | Learn it 1: `init`, `project add`, `project list` (terminal) |
| 6 | status | Learn it 2: `meta git status` (terminal) and how to read it |
| 7 | fan-out | Learn it 3: `meta exec --all --parallel`, results table (terminal) |
| 8 | scope | Learn it 4: directory-aware scope as nested boxes |
| 9 | worktrees | Learn it 5: `worktree add --all` (terminal), one branch in every repo |
| 9a | labels | Learn it 6: `repo label add`, `--label` selection (terminal); sits between worktrees and agents |
| 10 | agents | Worktree per agent (teal statement slide) |
| 11 | harness | Four ways to make an agent fluent in meta |
| 12 | roles | What developers and product managers each get |
| 13 | use-cases | Four use cases |
| 14 | extend | Manifest plugin, protocol plugin, module |
| 15 | hub | Pattern 1: hub workspace |
| 16 | nested | Pattern 2: nested workspaces |
| 17 | shared-tooling | Patterns 3 and 4: tooling repo, modules |
| 18 | opt-in | What is opt-in (table) |
| 19 | decision | Should you use meta: three-question flow (light slide) |
| 20 | not-for | When not to use it (light slide) |
| 21 | not-yet | Not well suited yet (light slide) |
| 22 | start | Five commands to start (terminal) and the cast |

Slides 1 to 9, 12, 19 and 22 were rebuilt in the redesign; the other ten were restyled afterwards to match: the cast now also appears on `agents`, `use-cases`, `hub` and `nested` (hub glides into nested), and the tables and limits slides share the newer heading and card styles.

## Files and the two renderers

Source of truth is this folder. Edit `slides/<id>.html` and `deck.json` (index: `order`, `sections`, `faces`).

- **Local viewer:** `make slides` builds and opens the deck (it runs `python3 docs/presentation/build.py`, which writes `index.html`, then opens that file in a browser). The viewer implements fade, push and magic transitions, all `data-build-in` kinds (auto-played, staggered by order), straight `<x-connector>` arrows, and `<x-embed>` as a sandboxed iframe that restarts when its slide is shown. Keys: arrows, space, Home, End, `n` notes, `f` fullscreen. Click the right or left half to move.
- **claude.ai Slides artifact:** https://claude.ai/artifact/ERpQGeYPdoHxamawL7BH1k (private to the user). It is the published copy; it can drift from this folder. Republish with the Artifact tool using `url` above, `root` = a folder containing `project/deck.json` and `project/slides/*.html`, and `file_path` = `project/deck.json`. Recreate the `project/` layout from this folder each time. Do not publish `index.html`.

## Slide format rules (the claude.ai subset; keep slides compatible with both)

- One `<section id="<id>">` per file, fixed **1920x1080** canvas, all styles inline, border-box sizing.
- Allowed: `px` lengths, hex or rgb colors, flex or grid layout, `position:absolute` pins, `<h1-3>`, `<p>`, `<ul>`, `<div>`, `<table>`, `<b>`, `<span style="color">`, `<x-connector>`, `<x-shape>`, `<x-icon>`, `<svg aria-label>` (may animate), `<x-embed>` (pinned, direct child of the section, 16 KB or less, 8 or fewer per slide), `data-build-in="fade|rise|drop|left|right|scale|pop N auto"` (pinned children only), `data-transition="fade|push|magic"`.
- Not allowed: classes, `<style>` or `<script>` outside an `<x-embed>`, `margin`, `z-index`, `em`/`var()`, external images or `data:` URIs.
- Text no smaller than 24px; 128px slide padding; keep content above y=952 and left of x=1792.
- Palette: bg `#0f1419`, cards `#182028`, borders `#2a3540`, text `#e8ebe6`, muted `#9aa7b2`, accents teal `#5ad1b8` and orange `#f0a35e`. Light slides: `#e9ebe4`. Cast: api `#5ad1b8`, web `#f0a35e`, docs `#c4a7ff`, infra `#7cc4ff`, sdk `#ffd98a`, worker `#ff9aa8`. Fonts: Space Grotesk (headings), Inter (text), JetBrains Mono (code).
- The local viewer does not draw `<x-icon>`, `<x-shape>` or routed connectors; the deck avoids them.
- Full spec: read `artifact-type/reference/format.md` via the Artifact `read` action on the artifact URL.

## How to verify (do not use keystroke injection or screen capture)

Use the cached headless Chromium directly:

```bash
H=$(find ~/Library/Caches/ms-playwright -name chrome-headless-shell -type f | head -1)
# URL fragments are ignored by --screenshot, so force the slide number in a temp copy:
sed "s#parseInt(location.hash.slice(1))||1#N#" docs/presentation/index.html > /tmp/tN.html
"$H" --no-sandbox --disable-gpu --window-size=1920,1080 --virtual-time-budget=9000 \
  --screenshot=/tmp/sN.png "file:///tmp/tN.html"
```

Then Read the PNG. For overflow, append a script that calls `go(k,1)` per slide and measures each element's bottom and right edge, with build animations disabled; flag anything past 952px or 1792px. Measure only after transitions settle: a mid-flight `rise` or magic move reads as a false overflow.

Verified on 2026-11-10: all 22 slides pass the overflow check, forward and backward navigation through every transition raises no script errors, and the 12 rebuilt slides plus `hub` were viewed as screenshots. The claude.ai renderer was **not** visually checked after republishing; magic move, `auto` builds and `<x-embed>` behave per its spec but only the local viewer was observed.

Gotchas already hit:
- Inline `display:flex` on a section beats stylesheet `display:none`; the viewer hides inactive slides with `!important`.
- Headless virtual time does not flush `requestAnimationFrame`; the viewer forces a synchronous reflow instead.
- The viewer once used content-box sizing, which made padded pinned cards overlap; the claude.ai renderer is border-box and the viewer now matches.
- A terminal embed loops, so a screenshot can catch it mid-typing.

## Verified vs unverified claims

Terminal output on `setup`, `status`, `fan-out` and `worktrees` is **trimmed from real runs of meta 0.83.0** (`target/debug/meta`) on a six-repo demo workspace built from local git repos. The git addresses (`git@github.com:acme/...`), the `acme` name, the `services/` folder on `scope`, and the drift labels on `problem` are illustrative. `meta init` writes `.metarepo`; older workspaces (this repo included) use `.meta`.

Sources for the rest: `README.md`, `docs/PRODUCT.md`, `docs/WORKTREE.md`, `docs/HARNESS_INTEGRATION.md`, `docs/MODULES.md`, `docs/PLUGIN_DEVELOPMENT.md`, `docs/DISABLING_PROJECTS.md`.

Still verify before presenting as fact:
- **"No profile for cloning only a subset"** (not-yet slide): labels (docs/LABELS.md) narrow commands but do not affect cloning; the cloning claim is based on a doc grep only.
- **Shared tooling pattern** (shared-tooling slide): inferred from this repo listing `.claude`, `.github`, `scripts` as projects.
- **`meta --root -w git status`** (nested slide): flags are real; not run against a real nested workspace.
- **`setup` success lines** were captured adding local repos; adding by git URL prints clone progress as well.
- Module gaps come from the "Out of scope" list in `docs/MODULES.md`.

## Improvement backlog (suggested, in priority order)

1. Open the republished artifact in Present mode and confirm magic move, builds and the terminal embeds there.
3. Add a slide on the plugin protocol (one JSON message exchange) using `docs/PLUGIN_PROTOCOL_V1.md`.
4. Pull the version badge from `Cargo.toml` at build time instead of the hardcoded "v0.85.0" on the cover.
5. Add a print/PDF export to `build.py` if the user wants a PDF (embeds flatten to one frame).

## Rules for the working agent

From the project and user CLAUDE.md files:
- Stage and commit finished work automatically; use GPG-signed commits (`git commit -S`), then verify with `git log --show-signature -1`. Do not add `timeout` (not installed on this Mac).
- Conventional Commits format, shell-safe (no backticks, exclamation marks, angle brackets, em-dashes or double quotes in the message). No agent attribution lines.
- If a package version changes, bump it in the commit; docs-only changes do not need a bump. Never push tags without user approval.
- Never invent statistics or quotes; label anything illustrative.
- Do not create or push to external services without the user's say-so. Republishing the artifact is allowed (it stays private); sharing is the user's call.
- When mentioning GitHub issues or PRs, write them as embedded links built from the origin remote.
- Document plans and findings under `docs/` (plan docs go in `docs/plans/`, matching the existing format).
