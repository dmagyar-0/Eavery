# Eavery

An open-source, local-first **desktop agent for everyday office work** — built in
Rust, provider-neutral, with every action explained before it happens and
reversible after.

> Status: **pre-implementation.** This repo currently holds research and strategy.

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

## Shape of the thing

```
Tauri v2 shell  →  Rust core (ACP client)  →  goose | Claude Code | Codex
                        ├── MCP servers   ("Connectors")
                        ├── Agent Skills  ("Playbooks")
                        └── git-backed workspace  ("Undo")
```

Borrow the entire engine. Build the entire experience.

## The four bets

1. **No API key** — drive the AI the user already pays for, via ACP.
2. **Invisible git** — automatic checkpoints, one Undo button, nothing lost.
3. **The plan gate** — plain-English plans, reviewed before anything runs.
4. **One engine, two vocabularies** — Everyday and Developer mode, one toggle apart.

## Standards, not formats

Eavery commits to [MCP](https://modelcontextprotocol.io),
[ACP](https://zed.dev/acp), [Agent Skills](https://agentskills.io), and
`AGENTS.md`. No proprietary playbook or connector formats.

## Licence

MIT — see [LICENSE](LICENSE). (Apache 2.0 under consideration; see
`docs/03-vision.md` §10.)
