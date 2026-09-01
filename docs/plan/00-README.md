# Eavery Implementation Plan — Index and Rules for the Implementer

This folder is the complete hand-off for building Eavery v1. It was written so
that a separate session, possibly running a smaller model, can implement the
product without re-deriving the design. Read this file first, then follow the
reading order below. Do not skip `02-challenges.md`.

The strategy documents in `docs/01..03-*.md` explain *why*. These plan documents
explain *what* and *how*. When they disagree, the plan documents win, because
they were written later and with verified technical facts (September 2026).

## Reading order

| # | File | What it gives you | Read when |
|---|---|---|---|
| 00 | `00-README.md` | This index and the working rules | First |
| 01 | `01-implementation-plan.md` | Scope of v1, locked decisions, milestones with exit tests | Before starting |
| 02 | `02-challenges.md` | The ten hardest problems and the chosen solution for each | Before starting |
| 03 | `03-architecture.md` | Crate layout, core types, event model, IPC contract, on-disk layout | Before M0 |
| 04 | `04-acp-engines.md` | ACP protocol cheat sheet, engine launch matrix, Rust SDK usage, fallback | M0–M1 |
| 05 | `05-git-journal.md` | Checkpoint and Undo design on top of libgit2 | M2 |
| 06 | `06-plan-gate-permissions.md` | Plan → approve → execute loop, permission policy on the irreversibility axis | M4 |
| 07 | `07-ui-vocabulary.md` | Screens, the Everyday/Developer dictionary, UI rules, digest | M3, M5 |
| 08 | `08-onboarding-packaging.md` | Engine detection, zero-key first run, BYO-key, Ollama, bundling, updater | M7 |
| 09 | `09-documents-playbooks.md` | Document MCP server (docx/xlsx/pdf/pptx) and Agent Skills as Playbooks | M6 |
| 10 | `10-task-breakdown.md` | Ordered task list with IDs, dependencies, and acceptance criteria | Every day |
| 11 | `11-testing-ci.md` | Test strategy, the fake ACP agent, CI workflow | M0 onward |
| — | `REVIEW-2026-09.md` | Independent review: verified claims, strategic issues, spec bugs found and fixed in these documents, the S0 spikes | Before S0 |

## Working rules (non-negotiable)

1. **Work the task list in order.** `10-task-breakdown.md` is ordered by
   dependency. Do not start a task whose dependencies are not marked done.
   Mark each task done in that file with the commit hash when finished.
   The S0 spikes come first and have pass/fail lines; a failed spike is a
   founder decision, not something to code around.
2. **One task, one commit.** Small commits with the task ID in the subject,
   for example `M2-T03: journal: create checkpoint before turn`.
3. **`cargo check` after every file you touch. `cargo test` before every commit.**
   Never commit code that does not compile. Never push a commit with failing tests.
4. **Do not invent APIs.** When you use a crate, open its docs on docs.rs for the
   exact version pinned in `Cargo.toml`. If a documented example in this plan
   does not compile against the pinned version, the crate changed: read the crate
   docs, adapt, and record what you changed in `docs/plan/CHANGELOG-plan.md`.
5. **Do not change a locked decision** (listed in `01-implementation-plan.md`
   §3) without writing the reason into `docs/plan/CHANGELOG-plan.md`. Prefer
   working around a problem inside the decision over changing the decision.
6. **Build the terminal path before the GUI path.** Every core feature must be
   drivable from `eavery-cli` first. It is far easier to debug a Rust binary
   than a Tauri webview, and the tests run in CI without a display.
7. **Trust features are not optional.** A checkpoint must exist before any
   agent turn may run. If checkpointing fails, the turn does not run. There is
   no "skip checkpoint" flag anywhere in the code.
8. **Never proxy or store another product's auth tokens.** Eavery launches the
   user's own agent CLI, which authenticates itself. Eavery never reads
   `~/.claude`, `~/.codex/auth.json`, or similar credential files.
9. **Everything the user sees goes through the vocabulary layer.** No raw
   protocol strings, tool names, or stack traces in Everyday mode. See
   `07-ui-vocabulary.md`.
10. **When stuck for more than an hour on one task**, write a note in
    `docs/plan/CHANGELOG-plan.md` describing what you tried, choose the fallback
    listed for that task (most tasks in `10-task-breakdown.md` have one), and
    move on. Do not stall the whole project on one crate quirk.

## Toolchain

- Rust stable (1.85 or newer, edition 2024). Install with `rustup`.
- Node 20 LTS or newer, `pnpm` (or `npm`) for the desktop frontend.
- Tauri v2 CLI: `cargo install tauri-cli --version "^2"`.
- Platform prerequisites for Tauri: see https://v2.tauri.app/start/prerequisites/
  (Linux needs `libwebkit2gtk-4.1-dev`, `libssl-dev`, `libgtk-3-dev`,
  `libayatana-appindicator3-dev`, `librsvg2-dev`; macOS needs Xcode CLT;
  Windows needs the WebView2 runtime and MSVC build tools).
- For manual engine testing only (not required by CI): at least one of
  `goose`, `claude`, `codex`, `gemini` installed and logged in.

## Definitions

- **Engine**: an ACP agent process (goose, Claude Code adapter, Codex adapter,
  Gemini CLI) that Eavery spawns and talks to over stdio.
- **Project**: a folder on disk chosen by the user. Everyday word for a workspace.
- **Journal**: the hidden git repository that records checkpoints for a Project.
- **Checkpoint**: a git commit in the Journal. Everyday word for commit.
- **Turn**: one `session/prompt` request and everything the engine does until it
  returns a stop reason.
- **Plan gate**: the step between the user's request and execution where the
  engine produces a plan and the user approves it.
- **Connector**: an MCP server. **Playbook**: an Agent Skill folder with `SKILL.md`.
- **Concurrency model**: at most one Turn runs per Project at a time. Several
  Projects may be open at once, each with its own engine process, Journal,
  and permission queue. `core://event` `seq` is global across Projects.
