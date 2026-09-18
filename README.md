# Eavery

An open-source, local-first **desktop agent for everyday office work** — built in
Rust, provider-neutral, with every action explained before it happens and
reversible after.

> Status: **M2 done bar its exit test against a real engine; M3's screens
> are in; M4's policy, prompts, plan gate and plan parser are in, and the
> two-phase turn that uses them is next.** The Rust workspace, the scriptable
> ACP test agent, the ACP client, the engine table and health checks, the
> git-backed Journal, the store, the turn engine and the headless CLI are in
> and tested, and so is the desktop app: twenty-three commands, the event
> stream, the generated types, and the window — Home, the three-pane Project
> screen with the transcript, the permission dialog, the history with Undo and
> Redo, Settings with the mode toggle and the assistants, and a Diagnostics
> panel over the log. From the window or a terminal you can open a folder,
> ask an assistant to change it, see what changed, and undo it. The
> permission policy now follows the full decision table, with "always"
> remembered per Project where the table allows it. What is not there yet:
> the plan → approve → execute turn and its plan card (M4-T05 onwards),
> Everyday-mode copy throughout (M5), the Documents tree (M5), and onboarding
> (M7). The plan lives in [`docs/plan/`](docs/plan/00-README.md) and the task
> list with it.

## The thesis

A coding agent is already a general computer agent — read, run tools, edit,
verify, iterate. What makes it feel developer-only is vocabulary and chrome, not
capability. Eavery keeps the engine and replaces the vocabulary.

Anthropic's own data supports this: of 1.2M Claude Cowork sessions across 600k+
organisations, **>90% had nothing to do with software development**.

## Documents

| | |
|---|---|
| [`docs/01-landscape.md`](docs/01-landscape.md) | What exists in open source (Aug 2026), assessed — goose, Codex, opencode, Kiro, LangChain, Rig — plus the standards (MCP, ACP, Agent Skills) and the provider-policy constraint that shapes the product. |
| [`docs/02-building-blocks.md`](docs/02-building-blocks.md) | Reference architecture, layer by layer, with build / borrow / buy calls. |
| [`docs/03-vision.md`](docs/03-vision.md) | Positioning, differentiators, wedge, moat, roadmap, monetisation, failure modes. |
| [`docs/plan/00-README.md`](docs/plan/00-README.md) | **Implementation plan** (Sept 2026): scope, locked decisions, the hardest problems and their solutions, architecture, ordered task list, and test strategy. Start here to build. |
| [`docs/plan/REVIEW-2026-09.md`](docs/plan/REVIEW-2026-09.md) | Independent review of all of the above: verified claims, strategic issues, spec bugs, and the three spikes to run before committing to the build. |

## Building it

```sh
cargo build --workspace
cargo test --workspace

# Which assistants are on this computer, and would they work right now.
cargo run -p eavery-cli -- engines

# One prompt through the scriptable test agent, end to end. No Project, no
# history: this is the engine-level tool.
mkdir -p /tmp/demo && cargo run -p eavery-cli -- prompt --engine fake \
  --script crates/eavery-core/tests/scripts/hello.json \
  --cwd /tmp/demo "write some notes"
```

The whole loop, on a folder that is protected from the moment it is opened.
Use a copy of a real folder, not the real one, until M2's exit test has been
run against an engine you trust:

```sh
mkdir -p /tmp/project && echo FY25 > /tmp/project/report.txt

cargo run -p eavery-cli -- project open /tmp/project
cargo run -p eavery-cli -- run --project /tmp/project --engine fake \
  --script crates/eavery-core/tests/scripts/hello.json "write some notes"

cargo run -p eavery-cli -- history --project /tmp/project
cargo run -p eavery-cli -- diff --project /tmp/project <checkpoint>
cargo run -p eavery-cli -- undo --project /tmp/project
```

Swap `--engine fake --script ...` for `--engine goose` (or whatever
`eavery-cli engines` says is ready) to drive a real assistant. The database
and the journals live in the platform's data directory; `--data-dir` puts
them somewhere else, which is what the tests do.

Progress is tracked in [`docs/plan/10-task-breakdown.md`](docs/plan/10-task-breakdown.md);
anything where reality differed from the plan is in
[`docs/plan/CHANGELOG-plan.md`](docs/plan/CHANGELOG-plan.md).

| Crate | What it is |
|---|---|
| `eavery-core` | Domain model, the one event stream, the `Engine` contract. Depends on no engine. |
| `eavery-acp` | ACP client: spawns an engine, maps its stream, answers its requests. |
| `eavery-engines` | Engine table, discovery, health checks. |
| `eavery-fake-agent` | A scriptable ACP agent. The primary test double. |
| `eavery-cli` | Headless driver. Every core feature is built here before the GUI. |
| `eavery-docs-mcp` | The document Connector. Arrives in M6. |
| `apps/desktop` | The Tauri v2 shell: the window, the generated types, the screens. See its [README](apps/desktop/README.md). |

## Shape of the thing

```
Tauri v2 shell  →  Rust core (ACP client)  →  goose | Claude Code | Codex
                        ├── MCP servers   ("Connectors")
                        ├── Agent Skills  ("Playbooks")
                        └── git-backed workspace  ("Undo")
```

Borrow the entire engine. Build the entire experience.

## The four bets

1. **Invisible git** — automatic checkpoints of the whole folder (including the
   user's own edits), one Undo button, nothing lost.
2. **The plan gate** — plain-English plans, reviewed before anything runs.
3. **One engine, two vocabularies** — Everyday and Developer mode, one toggle apart.
4. **No API key** — drive the AI the user already pays for, via ACP. Honest
   caveat: this needs the vendor's CLI on the machine. Eavery downloads Codex
   CLI and its ACP adapter itself (terminal-free with a ChatGPT account); the
   Claude and Gemini adapters need Node, and Anthropic has said its
   subscription billing for ACP use will change. goose already offers the same
   subscription sign-in, so this is table stakes, not the moat.

The moat is the first three. See `docs/plan/REVIEW-2026-09.md` §3.

## Standards, not formats

Eavery commits to [MCP](https://modelcontextprotocol.io),
[ACP](https://zed.dev/acp), [Agent Skills](https://agentskills.io), and
`AGENTS.md`. No proprietary playbook or connector formats.

## Licence

MIT — see [LICENSE](LICENSE). (Apache 2.0 under consideration; see
`docs/03-vision.md` §10.)
