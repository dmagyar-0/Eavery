# 01 — Implementation Plan: Scope, Locked Decisions, Milestones

## 1. What v1 is

v1 delivers Phase 0 and Phase 1 of the roadmap in `docs/03-vision.md` §7, plus
the minimum of Phase 2 needed to demonstrate the wedge:

- A desktop app (macOS, Windows, Linux) that opens a folder as a Project.
- The user types a request. Eavery drives an engine over ACP to plan it, shows
  the plan in plain English, waits for approval, executes, and shows a digest.
- Every turn is bracketed by automatic checkpoints. One Undo button restores
  any checkpoint. Nothing is ever lost.
- Engines: the user's own Claude Code, Codex CLI, or Gemini CLI (zero API key),
  or goose with a bring-your-own key, or goose with Ollama (fully local).
  For Codex, Eavery downloads Codex CLI and the `codex-acp` adapter itself
  (both ship native binaries) and launches the browser sign-in from inside the
  app, so a person with only a ChatGPT account never opens Terminal. Claude
  and Gemini need Node for their adapters and are labelled as such.
- Everyday mode by default, Developer mode one toggle away.
- A built-in document Connector (MCP server) for reading and writing `.docx`,
  `.xlsx`, and reading `.pdf` and `.pptx`.
- Playbooks: Agent Skills folders discovered from the Project and from the
  user's library, listed in the UI, and made available to the engine.

## 2. What v1 is not

Do not build any of the following in v1. They are listed so the implementer
does not drift into them:

- A Playbook registry, marketplace, or sharing.
- Scheduled or event-triggered runs.
- Team features, policy sync, SSO, audit export beyond the local audit log.
- A custom agent loop or a custom LLM provider layer. The engine is always an
  external ACP process.
- Email, calendar, Slack, Drive Connectors. Users can add any MCP server
  through Settings; Eavery does not ship them in v1.
- Writing `.pptx` files. v1 reads them only.
- Mobile, web, or remote sessions.
- A visual diff of binary documents. v1 shows a plain-language change summary
  (file added / changed / removed, plus text diffs for text files).

## 3. Locked decisions

These were decided with verified facts in September 2026. Change only with a
written reason in `CHANGELOG-plan.md`.

| ID | Decision | Reason |
|---|---|---|
| D1 | Rust workspace with a Tauri v2 desktop app and a React + TypeScript + Vite frontend | Tauri v2 is stable (2.11.x, July 2026). React/TS/Vite is the most widely documented combination; fewest surprises for the implementer. |
| D2 | ACP is the only engine interface. Eavery never links an engine as a library. | Makes engines swappable and keeps auth with the user's own CLI. |
| D3 | Use the `agent-client-protocol` crate, version 2.x, protocol version 1 | 2.0.0 released July 2026 with a builder API. Protocol v1 is what all four engines speak. Fallback: hand-rolled JSON-RPC over stdio using `agent-client-protocol-schema` types (see `04-acp-engines.md` §7). |
| D4 | Journal uses `git2` with `vendored-libgit2`, and the git directory lives outside the Project folder | No dependency on a system git binary. No `.git` folder inside the user's Documents/OneDrive folder. See `05-git-journal.md`. |
| D5 | Undo is forward-only: restoring a checkpoint creates a new commit whose tree equals the target. Never `reset --hard`, never rewrite history. | "Nothing ever lost" must be literally true. |
| D6 | The plan gate is enforced by the client, not trusted to the engine | The client owns `session/request_permission`. During planning it rejects every mutating tool call. See `06-plan-gate-permissions.md`. |
| D7 | Permission prompts are decided by irreversibility class, not by tool type | Reversible local edits inside the Project are auto-allowed (they are checkpointed). Outbound and destructive actions always ask. |
| D8 | Persistence is SQLite via `rusqlite` with the `bundled` feature, one database per user in the Eavery data dir | Durable sessions and audit log without a server. |
| D9 | Vocabulary is a single dictionary applied at the UI boundary. One event model, two renderings. | One codebase, one session, two audiences. |
| D10 | Zero-key path spawns CLI adapters: `@agentclientprotocol/codex-acp` (downloaded by Eavery together with Codex CLI), `@agentclientprotocol/claude-agent-acp` and `gemini --experimental-acp` (user-installed, need Node). BYO-key and local paths use goose downloaded on first use. | Codex is the primary zero-key engine: native binaries, ChatGPT sign-in, no billing change announced. Anthropic's separate billing for ACP and Agent SDK use was paused on 16 June 2026 but not withdrawn. No single engine may be load-bearing. |
| D11 | `eavery-cli` exists from M0 and exercises every core feature headlessly | Testable in CI, debuggable without a webview. |
| D12 | A fake ACP agent (`eavery-fake-agent`) is the primary test double | Deterministic tests for streaming, permissions, plan parsing, and failure modes. |
| D13 | Frontend talks to Rust only through a small set of Tauri commands and one event channel (`core://event`) | Keeps the UI a pure renderer of the core event stream. |
| D14 | Licence stays MIT for v1 | Changing licence is a founder decision, not an implementer decision. |
| D15 | Reads by the engine through `fs/read_text_file` are allowed anywhere the engine's own tools could read (outside the Project included). Writes through `fs/write_text_file` are Project-only. | Reads are not dangerous and the engine has its own read tools anyway; refusing them breaks Playbooks in `~/.eavery/playbooks`. |
| D16 | Restore always takes a checkpoint of the current work tree first. | Otherwise the user's own edits since the last checkpoint are overwritten by Undo, which breaks D5. |

## 4. Milestones

Each milestone has an exit test. Do not start the next milestone until the exit
test passes and is recorded in `10-task-breakdown.md`.

### S0 — Spikes (week 0–3, before any milestone)
Three throwaway experiments with pass/fail lines, from `REVIEW-2026-09.md` §7.
They are not product code; delete them when done.
1. **Terminal-free zero-key with ChatGPT.** Download Codex CLI and
   `codex-acp` binaries from a throwaway Tauri window, launch `codex login`
   from it, drive one ACP prompt, confirm the read-only mode blocks writes.
   Pass: a person with only a ChatGPT Plus account and no Terminal reaches a
   completed prompt.
2. **Journal on a real synced folder.** Detached git dir on a OneDrive or
   iCloud folder with ~500 MB of mixed Office files, Excel holding one file
   open. Checkpoint, edit, restore, undo a hand edit. Pass: byte-identical
   restore, locked file reported, no `.git` visible, no full re-hydration.
3. **`.docx` find-and-replace preserving formatting** on ten real documents
   from a finance or ops person. Pass: Word opens all ten without repair.
In parallel: sit with three to five finance or ops people and watch them try
a month-end task with Claude Code or Cowork on their own files.
**Exit:** all three pass, or a written founder decision on what changes.
If spike 1 fails, see `REVIEW-2026-09.md` §7 option B.

### M0 — Skeleton and fake engine (target: week 1–2)
Workspace compiles on all three platforms in CI. `eavery-fake-agent` speaks ACP.
`eavery-cli prompt --engine fake "hello"` streams the fake agent's reply.
**Exit test:** CI green on Linux, macOS, Windows; the CLI round-trips a prompt
through the fake agent including one permission request.

### M1 — Real engines from the CLI (week 2–4)
Engine discovery finds installed CLIs. The CLI can run a prompt through goose,
Claude Code adapter, Codex adapter, and Gemini CLI, with permission prompts
answered in the terminal. Session events are normalised into `CoreEvent`.
**Exit test:** the same prompt ("list the files in this folder and summarise
them") completes through at least two real engines with no code change between
them.

### M2 — Journal (week 4–6)
Open a folder as a Project, create the hidden Journal, checkpoint before and
after a turn, list checkpoints, show a change summary, restore any checkpoint.
**Exit test (roadmap Phase 0 exit):** run a turn that edits files through a
real engine, Undo, and confirm the folder is byte-identical to the pre-turn
checkpoint. Then Redo (restore the post-turn checkpoint) and confirm again.

### M3 — Desktop shell, Developer mode (week 6–9)
Tauri app: project list, chat with streaming, tool call trail, permission
dialogs, checkpoint list with Undo. Raw vocabulary is acceptable here.
**Exit test:** a developer completes M2's exit test entirely from the GUI.

### M4 — Plan gate and permission policy (week 9–11)
Two-phase turn: plan, approve/edit, execute. Permission policy on the
irreversibility axis. Audit log.
**Exit test:** with the fake agent scripted to attempt an edit during planning,
the edit is refused and the plan still renders. With a real engine, a request
that would send data outside the machine (an MCP fetch) triggers a prompt;
a local edit inside the Project does not.

### M5 — Everyday mode (week 11–13)
Vocabulary dictionary applied. Digest after each run. Errors rendered as next
actions. Documents view instead of file tree. Developer toggle.
**Exit test:** the implementer completes "rename every 'FY25' to 'FY26' in
the three text notes in this folder and tell me what you changed" entirely in
Everyday mode: reads the plan, approves, reads the digest, undoes it, and
confirms no protocol word appears on screen. (The Word-document version of
this task needs the docs Connector from M6; a non-technical tester is not
asked to sit through the engine scripting a `.docx` edit by hand.)

### M6 — Document Connector and Playbooks (week 13–16)
`eavery-docs-mcp` server with docx/xlsx read+write, pdf/pptx read. Playbook
discovery and injection. Connector management UI.
**Exit test (roadmap Phase 1 exit):** a non-technical tester completes
"rename every 'FY25' to 'FY26' in these three Word documents and tell me what
you changed", reads the plan, approves, reads the digest, then undoes it,
without help. The edit is done by the docs Connector rather than by the
engine's own scripting, and Word opens the files without repair.

### M7 — Onboarding, packaging, durability (week 16–19)
First-run flow: detect engines, zero-key path (including downloading Codex
CLI and `codex-acp` and launching `codex login`), BYO-key path with goose
download, Ollama path. Session persistence across restart. Installers for the
three platforms with the Tauri updater.
**Exit test:** a fresh machine with nothing installed and a ChatGPT account
goes from installer to first completed task without opening Terminal or typing
an API key. A fresh machine with only Claude Code installed does the same
(Node permitted). A fresh machine with nothing installed goes from installer
to first task with an Anthropic or OpenAI key, or with Ollama, in under ten
minutes.

## 5. Team shape and pace

This plan assumes one implementer session working sequentially. Milestone
weeks are guidance, not commitments; a realistic total for what is listed is
30 to 40 weeks, with M6-T05 (`.docx` edits across formatting runs) and M7
packaging the most underestimated. The order matters more than the dates:
S0 → M0 → M1 → M2 must be done before any UI work because they de-risk the
things that can kill the project (onboarding, engine interop, undo correctness).

## 6. Where the risk is

Ranked, highest first. Each has a section in `02-challenges.md`.

1. Onboarding without a terminal (S0, M7).
2. Engine interop and auth policy (M1, M7).
3. Undo correctness on real office folders, including cloud-synced ones (S0, M2).
4. Enforcing plan-before-action on engines Eavery does not control (M4).
5. GUI process environment on macOS and Windows (M1, M3).
6. Document fidelity for `.docx`/`.xlsx` (S0, M6).
