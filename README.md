# Eavery

An open-source, local-first **desktop agent for everyday office work** — built in
Rust, provider-neutral, with every action explained before it happens and
reversible after.

> Status: **pre-implementation.** This repo holds research, strategy, and a
> complete implementation plan in [`docs/plan/`](docs/plan/00-README.md).

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
