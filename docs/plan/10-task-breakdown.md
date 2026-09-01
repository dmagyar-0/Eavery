# 10 — Task Breakdown

Ordered by dependency. Each task: what to build, where, how you know it is
done, and a fallback if the primary approach fails. Sizes: S (< half a day),
M (about a day), L (2–3 days). Mark tasks done by replacing `[ ]` with `[x]`
and appending the commit hash.

Conventions: crate paths are relative to the repo root. "CLI" means
`crates/eavery-cli`. "Fake" means `crates/eavery-fake-agent`.

---

## M0 — Skeleton and fake engine

- [ ] **M0-T01 (S)** Create the Cargo workspace from `03-architecture.md` §1–2
  with empty lib crates and `fn main() {}` binaries. `rust-toolchain.toml`,
  `.gitignore` (target, node_modules, dist, `*.sqlite`), `rustfmt.toml`,
  `clippy.toml`. Done when `cargo build --workspace` passes.
- [ ] **M0-T02 (S)** CI: `.github/workflows/ci.yml` per `11-testing-ci.md` §5,
  Rust only for now (no Tauri yet). Done when the workflow is green on all three OSes.
- [ ] **M0-T03 (M)** `eavery-core::model` and `eavery-core::event` types from
  `03-architecture.md` §3–4, with `serde` and `ts-rs` derives. Unit test that
  every `CoreEvent` variant round-trips through JSON. Done when the test passes.
- [ ] **M0-T04 (M)** `eavery-core::engine` trait and `RawAgentEvent` enum from
  `03-architecture.md` §5 and `04-acp-engines.md` §6. No implementation yet.
- [ ] **M0-T05 (L)** Fake agent: an ACP agent binary that reads a JSON script
  (`11-testing-ci.md` §2) and replays it: `initialize` reply, `session/new`
  reply with optional modes, and for each `session/prompt` a list of actions
  (`text`, `thought`, `tool_call`, `tool_call_update`, `plan`,
  `request_permission` expecting a decision, `fs_write`, `sleep_ms`, `stop`).
  Implement as hand-rolled JSON-RPC over stdio (it must not depend on the SDK
  so that SDK bugs are visible). Done when `echo` of a scripted text reply
  works via a manual `printf ... | fake-agent` test and unit tests cover
  request/response framing.
- [ ] **M0-T06 (L)** `eavery-acp::AcpEngine` implementing `Engine` with the
  2.x SDK (`04-acp-engines.md` §5): spawn from a `LaunchSpec`, initialize,
  session/new, prompt with streaming to `EventSink`, permission handler
  bridge, cancel, shutdown. Fallback: `04-acp-engines.md` §7 hand-rolled
  client. Done when an integration test runs a fake script with text, a tool
  call, and a permission request through `AcpEngine` and observes the events
  in order.
- [ ] **M0-T07 (M)** CLI: `eavery-cli prompt --engine fake --script <path> --cwd <dir> "<text>"`
  prints events as they arrive and answers permissions from the terminal
  (`a`/`r`). Done when the M0 exit test passes and is recorded here.

**M0 exit recorded:** ______

## M1 — Real engines from the CLI

- [ ] **M1-T01 (M)** `eavery-engines`: `EngineSpec` table from
  `04-acp-engines.md` §2–3, `LaunchSpec` resolution (explicit path, PATH,
  well-known locations per `08-onboarding-packaging.md` §2), Windows
  `npx.cmd` handling. Unit tests with a fake PATH. 
- [ ] **M1-T02 (S)** PATH fix on macOS/Linux via `fix-path-env` (or equivalent
  login-shell probe with 3 s timeout), called once at process start in CLI
  and desktop.
- [ ] **M1-T03 (M)** Health check (`04-acp-engines.md` §9) with timeouts and
  `EngineStatus` results; CLI command `eavery-cli engines` prints a table.
- [ ] **M1-T04 (M)** Manual verification against goose: configure goose with
  any provider, run the M1 exit prompt. Record the `modes` it advertises, and
  whether `mcpServers` in `session/new` are loaded, in `CHANGELOG-plan.md`.
- [ ] **M1-T05 (M)** Same for the Claude adapter (`claude-agent-acp`). Record
  the plan mode id and permission option kinds it sends.
- [ ] **M1-T06 (M)** Same for `codex-acp`. Record modes and approval behaviour.
- [ ] **M1-T07 (S)** Same for `gemini --experimental-acp`. If it is unusable
  on the tested version, mark the engine `experimental: true` (hidden behind
  Developer mode) and record why.
- [ ] **M1-T08 (S)** stderr capture ring buffer and `EngineCrashed` event with
  the last 50 lines; test by scripting the fake agent to exit mid-prompt.

**M1 exit recorded:** ______

## M2 — Journal

- [ ] **M2-T01 (M)** `Journal::open_or_create` with detached git dir,
  `info/exclude`, initial checkpoint. Tests 1, 6, 9 from `05-git-journal.md` §7.
- [ ] **M2-T02 (M)** `checkpoint` with size guard and trailers; `list`. Tests 2, 5, 7.
- [ ] **M2-T03 (M)** `diff` and `diff_worktree` producing `ChangeSet` with text diffs. Test on text and binary fixtures.
- [ ] **M2-T04 (L)** `restore` forward-only, per-file, lock-tolerant. Tests 3, 4, 8.
- [ ] **M2-T05 (S)** `unprotected()` and the guard constants; `open_project` size scan with `MAX_FILES` and `WARN_TOTAL_BYTES`.
- [ ] **M2-T06 (M)** `eavery-core::store`: SQLite open, migrations, CRUD for
  projects/sessions/turns/events/checkpoints/audit/settings. Tests with a temp db.
- [ ] **M2-T07 (M)** `eavery-core::turn` state machine in **direct mode only**
  (no plan gate yet): pre-checkpoint → prompt → post-checkpoint → digest.
  Permission handler = allow reads/reversible, ask via callback for the rest.
- [ ] **M2-T08 (M)** CLI: `project open <dir>`, `project list`, `run --project <id> --engine <id> "<text>"`,
  `history --project <id>`, `undo --project <id> [--to <cp>]`, `diff --project <id> <from> <to>`.
- [ ] **M2-T09 (S)** M2 exit test against a real engine, byte-compare with
  `diff -r` (or a Rust helper), recorded below with the engine used.

**M2 exit recorded:** ______

## M3 — Desktop shell (Developer mode)

- [ ] **M3-T01 (M)** `pnpm create tauri-app` (react-ts) into `apps/desktop`;
  add `src-tauri` to the workspace; app builds and shows a window on all three
  OSes in CI (build only, no run).
- [ ] **M3-T02 (M)** `ts-rs` bindings generation into `apps/desktop/src/types.ts`
  via a `cargo test` in `eavery-core`; CI fails if the generated file is stale.
- [ ] **M3-T03 (M)** Tauri state: an `AppCore` struct wrapping store, journal
  cache, engine registry, event broadcast; `core://event` emission with `seq`.
- [ ] **M3-T04 (L)** Commands from `03-architecture.md` §7: projects, engines,
  `start_turn` (direct), `answer_permission`, `cancel_turn`, checkpoints,
  `list_events`, settings. Each command is a thin call into `eavery-core`.
- [ ] **M3-T05 (M)** Frontend `ipc.ts`, `events.ts`, `store.ts` with gap re-fetch.
- [ ] **M3-T06 (L)** Screens: Home, Project (three panes), Settings (mode +
  engines only). Raw strings acceptable but must go through `t()` from the start.
- [ ] **M3-T07 (M)** `Transcript`, `ToolCallRow`, `PermissionDialog` (queue), `Checkpoints` with Undo/Redo.
- [ ] **M3-T08 (S)** `Diagnostics` panel with log tail (tail the `tracing` file).
- [ ] **M3-T09 (S)** Kill children on exit (`kill_on_drop` plus explicit
  shutdown in Tauri's `RunEvent::Exit`).

**M3 exit recorded:** ______

## M4 — Plan gate and policy

- [ ] **M4-T01 (S)** Prompt templates and the tiny renderer (`06-plan-gate-permissions.md` §4). Unit tests for `{{#if}}`.
- [ ] **M4-T02 (M)** `policy::classify` and the decision table; `ConnectorRegistry` with the `outbound` flag. Unit tests for every row.
- [ ] **M4-T03 (M)** `PlanGateHandler` and `fs/write_text_file` refusal during planning.
- [ ] **M4-T04 (M)** Plan extraction parser with fallback; tests for valid JSON, invalid JSON, markdown list fallback.
- [ ] **M4-T05 (L)** Two-phase turn in `eavery-core::turn`: Planning → AwaitingApproval → Executing; mode switching via `set_mode` when a plan mode exists; cancel in each phase; digest with outbound and refused lists.
- [ ] **M4-T06 (M)** Audit log rows for every decision; `list_audit` command (Developer mode view).
- [ ] **M4-T07 (M)** UI: `PlanCard`, `approve_plan`/`reject_plan` commands, "always" storage.
- [ ] **M4-T08 (M)** Fake-agent scripts and tests from `06-plan-gate-permissions.md` §7.
- [ ] **M4-T09 (S)** M4 exit test with a real engine recorded below.

**M4 exit recorded:** ______

## M5 — Everyday mode

- [ ] **M5-T01 (M)** `vocab/dictionary.ts` complete per `07-ui-vocabulary.md` §2; `t()` with variables; mode toggle persisted.
- [ ] **M5-T02 (S)** `scripts/check-vocab.mjs` and CI step.
- [ ] **M5-T03 (M)** Everyday renderings: `ToolCallRow` one-liners, thoughts hidden, `Digest` component, error-as-next-action.
- [ ] **M5-T04 (M)** `DocumentsPane` with changed-file markers and OS open.
- [ ] **M5-T05 (S)** "Ask a question" direct mode with read-only intent.
- [ ] **M5-T06 (M)** Copy pass: every string in the app reviewed against the UI rules; no protocol words in Everyday mode.
- [ ] **M5-T07 (M)** Usability test with one non-technical person (the M5 exit test). Record observations and fix the top three problems before marking done.

**M5 exit recorded:** ______

## M6 — Document Connector and Playbooks

- [ ] **M6-T01 (M)** `eavery-docs-mcp` skeleton with `rmcp`, `--root`, `doc_info`, path guard, stdio test.
- [ ] **M6-T02 (M)** `xlsx_list_sheets`, `xlsx_read_range` (calamine).
- [ ] **M6-T03 (M)** `xlsx_write_cells` (umya) with validation round-trip; `xlsx_create` (rust_xlsxwriter).
- [ ] **M6-T04 (M)** `docx_read_text` (docx-rs).
- [ ] **M6-T05 (L)** `docx_replace_text` across runs with zip + quick-xml, preserving all parts; `docx_append_paragraphs`. Golden-file tests.
- [ ] **M6-T06 (M)** `pdf_read_text`, `pptx_read_text`.
- [ ] **M6-T07 (S)** Bundle the binary as `externalBin`; Eavery passes it in `session/new` for every engine; verify each engine lists its tools (record per engine).
- [ ] **M6-T08 (M)** Connectors settings UI and `connectors.json`; outbound flag.
- [ ] **M6-T09 (M)** Playbook discovery, validation, injection into the plan prompt; Settings → Playbooks list; composer menu.
- [ ] **M6-T10 (L)** Five bundled Playbooks, each run once with two engines; fix wording until both follow them.
- [ ] **M6-T11 (S)** M6 exit test recorded: Word opens the modified `.docx` without repair; Excel opens the `.xlsx`.

**M6 exit recorded:** ______

## M7 — Onboarding, packaging, durability

- [ ] **M7-T01 (L)** Onboarding screens and flow from `08-onboarding-packaging.md` §1 with background detection.
- [ ] **M7-T02 (M)** goose download with checksum, quarantine removal on macOS, version pin.
- [ ] **M7-T03 (M)** Keychain storage for keys (`keyring`), env injection for goose child only.
- [ ] **M7-T04 (S)** Ollama detection (`/api/tags`) and model picker.
- [ ] **M7-T05 (M)** Session durability (`08-onboarding-packaging.md` §7): `session/load` or summary prepend; mid-turn close recovery.
- [ ] **M7-T06 (M)** Tauri updater config and signing keys; release workflow on tags.
- [ ] **M7-T07 (M)** Installers built in CI for all three OSes; manual install test on a clean VM each.
- [ ] **M7-T08 (S)** README updated with install instructions and the unsigned-build caveat.
- [ ] **M7-T09 (S)** M7 exit test recorded for both a zero-key machine and a nothing-installed machine.

**M7 exit recorded:** ______

---

## Cross-cutting, do continuously

- Keep `CHANGELOG-plan.md` current whenever reality differs from these documents.
- Keep `BACKLOG.md` for ideas that are not v1.
- Every crate has `#![deny(unsafe_code)]` except where `git2` FFI needs otherwise (it does not).
- `cargo clippy --workspace --all-targets -- -D warnings` stays green.
