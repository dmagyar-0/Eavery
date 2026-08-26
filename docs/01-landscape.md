# Landscape: What Already Exists (August 2026)

Research snapshot for Eavery. Everything here was verified against primary sources
in August 2026. Links are in `#sources` at the bottom.

## 1. The short answer

There is **one** credible open-source Rust agent runtime you can build on today:
**goose**. Everything else is either the wrong language (opencode is Go, LangChain
is Python), proprietary (Kiro, Claude Code, Cowork), or a library rather than a
runtime (Rig).

The more important finding is that **you probably should not pick a single engine
at all.** Three open standards landed between December 2025 and April 2026 that
make the agent engine a swappable component:

| Standard | What it does | Governance | Adoption |
|---|---|---|---|
| **MCP** | Agent ↔ tools/data | Agentic AI Foundation (Linux Foundation), donated Dec 2025 | de-facto universal |
| **ACP** (Agent Client Protocol) | UI ↔ agent | Zed Industries, open spec | 25+ agents; JetBrains, Google, GitHub |
| **Agent Skills** (`SKILL.md`) | Portable procedural knowledge | agentskills.io open spec, Dec 2025 | ~40 products incl. Claude, Codex, Copilot, Cursor, goose |

Plus `AGENTS.md` (OpenAI, donated to AAIF) for project-level instructions.

**This is the strategic unlock.** If Eavery is an ACP *client* that speaks MCP and
consumes Agent Skills, then goose, Codex, Claude Code, and Gemini CLI are all
interchangeable backends. You are not betting the company on one engine, and you
inherit every capability improvement any of them ship.

## 2. Candidate engines, assessed

### goose — the recommended base
- **Rust**, Apache 2.0, ~53.5k stars, 500+ contributors, ~5,500 commits.
- Governed by the **Agentic AI Foundation** at the Linux Foundation (donated by
  Block, 2025). Neutral governance — no single vendor can pull the rug.
- Workspace of crates that map almost exactly onto what Eavery needs:
  - `goose` — core: agent loop, providers, config, sessions, OAuth, security, telemetry
  - `goose-server` — the `goosed` daemon; axum, REST + WebSocket
  - `goose-cli`, `goose-acp` — clients
  - `goose-mcp` — built-in MCP servers incl. a **computer controller**
- 15+ providers (Anthropic, OpenAI, Google, Bedrock, Azure, OpenRouter, Ollama).
- **goose 2.0 (April 2026) made ACP the default interface** — one server, many
  clients. This is the same architecture Eavery wants.
- Desktop app is Electron + React today, with a **migration to Tauri v2 underway**
  (discussion #7332). Meaning: the Rust-native desktop shell you want is a gap the
  upstream project has identified but not filled. That is a contribution opportunity
  and a differentiation window.

**Verdict:** best base. Rust, neutrally governed, ACP-native, permissively licensed.

### OpenAI Codex CLI — the sandbox and agent-loop reference
- **Rust (~96%)**, Apache 2.0, ~114k stars, 443 contributors.
- Rewritten TypeScript → Rust in June 2025; ~20M active users by Aug 2026.
- Best-in-class **sandboxing**: Seatbelt on macOS, Landlock + seccomp on Linux,
  restricted tokens on Windows.
- "Symphony" (April 2026) — a published spec for multi-Codex orchestration.
- Apache 2.0 means you can **read and reuse the sandbox code directly**.

**Verdict:** don't fork it as a base, but treat its sandbox as the reference
implementation, and support it as an ACP backend.

### opencode (SST) — the UX reference, wrong language
- **Go**, MIT, ~160k+ stars, 7.5M monthly developers, 75+ providers.
- Client/server: one local server drives TUI, desktop app, and IDE extensions —
  the server survives terminal disconnect.

**Verdict:** steal the architecture pattern and the provider-neutral positioning.
Do not depend on it — a Go core inside a Rust product is the worst of both.

### Kiro (AWS) — you asked specifically; the answer is no
- Kiro IDE is **proprietary**, a Code OSS (VS Code) fork. The spec engine and agent
  system are closed.
- **Kiro CLI is also proprietary.** It is the rebrand (Nov 2025) of
  `aws/amazon-q-developer-cli`, which *was* Rust and open source — but that repo is
  now maintenance-only (critical security fixes), and Amazon Q reaches end of
  support April 2027.
- Kiro is Bedrock-bound (routes Claude Sonnet for reasoning, Amazon Nova for
  throughput).

**Verdict: you cannot use Kiro as an agent inside Eavery.** But two things carry over:
1. **The archived Apache-2.0 Q CLI Rust source is legitimately forkable** for
   specific components, if anything there beats goose. Treat as a parts bin, not a base.
2. **The spec-driven pattern is the real prize, and it's free to copy.** Kiro's
   differentiator is refusing to go prompt→code: it generates requirements, a
   design doc, and a reviewable task list, and only executes after you approve.
   Its "agent hooks" fire agentic tasks on events.

   **For office workers this pattern is not a nice-to-have — it is the entire
   safety model.** A non-technical user cannot review a diff, but they *can*
   review a plan in plain English. See `02-building-blocks.md` §4.

### LangChain / LangGraph — wrong layer
- Python/TS. LangChain ~134k stars; `AgentExecutor` deprecated, maintenance until
  Dec 2026; LangGraph is now the recommended orchestration layer (durable execution,
  checkpointing, human-in-the-loop).
- Well-known criticisms: abstraction weight, dependency bloat, API churn, hidden
  control flow, hard debugging.

**Verdict:** not in the desktop core. Shipping a Python runtime inside a Rust
desktop app forfeits the reason to choose Rust. Relevant only if Eavery later adds
an optional server-side workflow engine. The ideas worth stealing — durable
execution, checkpointing, human-in-the-loop gates — should be implemented natively.

### Rust libraries (not runtimes)
- **Rig** (0xPlaygrounds, MIT, ~7.6k stars): 20+ providers, 10+ vector stores,
  OpenTelemetry GenAI conventions, WASM, MCP. Production users incl. Neon,
  Nethermind. It is a *library* — LLM clients, function schemas, response parsing.
  Orchestration is left to you.
- **AutoAgents** (Ractor actor model), **OpenFANG** ("agent OS").
- Measured Rust-vs-Python: ~5x lower peak memory (1.1GB vs 5.1GB), ~13x throughput.
  For an always-on desktop app competing with Electron, this is the whole argument.

**Verdict:** Rig is the fallback if you ever need to build the provider layer
yourself. goose's provider layer already covers it.

## 3. The competitive field is not empty

A category formed in H1 2026: **open-source Cowork desktops**. Known players:

- **Eigent** — Apache 2.0, multi-agent, 200+ MCP tools, local-first, SSO/RBAC.
  Currently the most complete OSS Cowork alternative.
- **Open Cowork** — simple desktop install, explicitly positioned at
  non-technical teammates.
- **OpenWorker / Openwork** — local-first, model-flexible, approval checkpoints,
  fully offline with Ollama.
- **Hermes Agent** (Nous Research) — persistent memory, scheduled work, a learning
  loop that creates and improves its own skills.

And the incumbent: **Claude Cowork** (Anthropic) — research preview Jan 2026, GA
April 2026, web + mobile July 2026. It is a GUI over Claude Code.

**The datapoint that validates the whole thesis:** Anthropic analysed 1.2M Cowork
sessions across 600k+ organisations and found **>90% had nothing to do with
software development** — usage concentrated in operations, marketing, finance,
legal, research.

Your instinct is correct and now empirically confirmed. The catch is that it is
confirmed *publicly*, so "coding agent for office workers" is no longer a secret.
Differentiation has to come from somewhere other than the idea. See `03-vision.md`.

## 4. The constraint that shapes the product

**Anthropic has banned third-party tools from using Claude subscription OAuth.**

- 9 Jan 2026 — server-side safeguards block subscription OAuth tokens outside the
  official Claude Code CLI.
- 19 Feb 2026 — docs updated: using OAuth tokens in third-party tools violates ToU.
  Free/Pro/Max tokens not permitted in third-party tools or the Agent SDK.
- 4 Apr 2026 — enforcement extended to all third-party agentic harnesses.
- Casualties included OpenClaw, opencode, NanoClaw.

**Implications for Eavery, and they are not small:**

1. You **cannot** let a user sign in with their Claude Pro/Max subscription. Any
   design that assumes this is dead on arrival and is also a ToU violation.
2. Anthropic support means **API keys and usage-based billing**. For an office
   worker who has never seen an API key, that is a brutal first-run experience —
   and it is the single biggest adoption barrier in this category. Note that the
   OpenWorker review above flags exactly this ("you need to be comfortable
   managing API keys").
3. **There is a legitimate path, and it is a differentiator:** don't proxy the
   subscription — *drive the user's own officially-installed agent over ACP.*
   Claude Code authenticates itself, under its own ToU, on the user's machine.
   Eavery is the client, not the harness. Same for Codex CLI.

   This turns the industry's biggest constraint into Eavery's onboarding
   advantage: "already have Claude or ChatGPT? Eavery uses it." No API key, no
   second bill, no ToU grey zone.
4. Policy risk is real and recurring. **Never let one provider be load-bearing.**
   Ship day one with: bring-your-own-agent (ACP), bring-your-own-key (Anthropic,
   OpenAI, Google, OpenRouter), and fully-local (Ollama).

## Sources

- [aaif-goose/goose](https://github.com/aaif-goose/goose) · [goose 2.0 / ACP](https://goose-docs.ai/blog/2026/04/08/goose-acp-and-new-tui/) · [Tauri migration #7332](https://github.com/aaif-goose/goose/discussions/7332) · [goose-server (DeepWiki)](https://deepwiki.com/aaif-goose/goose/5-server-and-api-layer-(goose-server))
- [Linux Foundation: Agentic AI Foundation](https://www.linuxfoundation.org/press/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation) · [OpenAI on AAIF](https://openai.com/index/agentic-ai-foundation/)
- [Zed: Agent Client Protocol](https://zed.dev/acp) · [ACP explained (Morph)](https://www.morphllm.com/agent-client-protocol)
- [Agent Skills spec (The New Stack)](https://thenewstack.io/agent-skills-anthropics-next-bid-to-define-ai-standards/) · [Agent Skills ecosystem 2026](https://agentman.ai/blog/agent-skills-ecosystem-report-2026)
- [OpenAI Codex (Wikipedia)](https://en.wikipedia.org/wiki/Codex_(AI_agent)) · [Codex CLI is Rust/Apache-2.0](https://toknow.ai/posts/openai-codex-cli-rust-coding-agent-open-source/) · [Agent sandbox comparison](https://www.developersdigest.tech/blog/ai-coding-agent-security-models-compared-2026) · [How Claude Code and Codex sandbox](https://medium.com/@Koukyosyumei/how-claude-code-and-codex-sandbox-untrusted-code-ba39b493046a)
- [What is OpenCode (DataCamp)](https://www.datacamp.com/blog/what-is-opencode)
- [Kiro CLI](https://kiro.dev/cli/) · [Upgrade to Kiro (AWS docs)](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/upgrade-to-kiro.html) · [aws/amazon-q-developer-cli](https://github.com/aws/amazon-q-developer-cli) · [Kiro developer guide](https://www.developersdigest.tech/blog/aws-kiro-developer-guide-2026)
- [LangGraph overview](https://docs.langchain.com/oss/python/langgraph/overview) · [Why developers say LangChain is bad](https://www.designveloper.com/blog/is-langchain-bad/)
- [Rig](https://mr.technology/payloads/rig-rust-llm-application-framework-june-2026) · [Rust agent ecosystem survey 2026](https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/)
- [Claude Cowork](https://www.anthropic.com/product/claude-cowork) · [VentureBeat: most Cowork users aren't coding](https://venturebeat.com/technology/anthropic-brings-claude-cowork-to-mobile-and-web-as-usage-data-shows-most-users-arent-coding)
- [Cowork alternatives (Composio)](https://composio.dev/content/best-claude-cowrk-alternatives) · [Open-source Cowork alternatives (Eigent)](https://www.eigent.ai/blog/best-open-source-claude-cowork-alternatives-2026)
- [The Register: Anthropic clarifies third-party ban](https://www.theregister.com/software/2026/02/20/anthropic-clarifies-ban-on-third-party-tool-access-to-claude/5014546) · [Anthropic bans third-party subscription OAuth](https://winbuzzer.com/2026/02/19/anthropic-bans-claude-subscription-oauth-in-third-party-apps-xcxwbn/)
- [Tauri v2 in 2026](https://rustify.rs/articles/rust-tauri-vs-electron-2026)
