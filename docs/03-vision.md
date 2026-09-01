# Product Vision & Strategy

## 1. The opportunity, stated honestly

Anthropic analysed 1.2M Claude Cowork sessions across 600k+ organisations:
**more than 90% had nothing to do with software development.** Usage concentrated
in operations, marketing, finance, legal, and research.

That validates the premise — agentic tools built for developers turn out to be
general work tools. But it validates it *publicly*, in a vendor blog post that
every competitor read. As of August 2026 the field already contains Claude Cowork
(GA, plus web and mobile), Eigent, Open Cowork, OpenWorker, and Hermes Agent.

**"Open-source Cowork" is not a strategy. It is a category with incumbents.**

The real gap is narrower and more defensible: every product in this category is
still built on the assumption that the user will manage API keys, tolerate an
agent editing files without a real undo, and supervise work they cannot read. For
an actual office worker, all three are disqualifying.

## 2. Vision

> **Eavery gives every office worker a colleague that does the work, not a chatbot
> that talks about it — on their own machine, with their own tools, where every
> action is explained before it happens and reversible after.**

## 3. Positioning

| | Claude Cowork | OSS Cowork clones | Coding agents | **Eavery** |
|---|---|---|---|---|
| Audience | knowledge workers | early adopters | developers | **office workers** |
| Model | Anthropic only | BYO key | BYO key | **BYO agent, key, or local** |
| Open source | no | yes | yes | **yes** |
| Onboarding | excellent | poor (API keys) | N/A | **BYO-agent: no key** (Codex terminal-free; Claude/Gemini need Node) |
| Undo | tool edits only, 30 days (`/rewind`) | limited | git (visible) | **whole folder, incl. user edits, forever (invisible git)** |
| Where data goes | Anthropic | model vendor | model vendor | **model vendor, or nowhere (Ollama)** |
| Supervision | permissions | permissions | diffs | **plain-English plan gate** |
| Runtime | cloud+desktop | Electron/Python | CLI | **Rust/Tauri, ~5MB** |

**One-line positioning:** *the open, local-first agent desktop that works with the
AI you already pay for.*

## 4. The four differentiators

Everything else is table stakes. These are the bets, in order of how much of
the moat they carry (see `docs/plan/REVIEW-2026-09.md` §3).

### 4.1 Invisible git — undo anything
Every project is a git repo the user never sees as git. Automatic checkpoint
before every action; one Undo button; nothing ever truly lost.

An agent with write access to real work is frightening until it is *provably*
reversible. This is the cheapest trust available — the hard engineering was done
by someone else in 2005 — and it is the precondition for anyone letting an agent
near a document that matters.

Be precise about the comparison: Claude Code and Cowork have `/rewind`
checkpoints, but they cover only edits made through the agent's own edit tools,
not shell commands and not the user's own edits, and they expire after 30 days.
Eavery's Journal covers the whole folder, whoever changed it, for as long as the
user wants.

### 4.2 The plan gate — supervision without literacy
Adapted from Kiro's spec-driven pattern. Plan in plain English → review and edit →
execute → digest. A non-technical user cannot audit a diff, but they can absolutely
read "I'll pull the three regional sheets, reconcile against the ledger, and draft
a summary — I won't email anything."

Critically, permission friction should track **irreversibility**, not action type:
near-silent on checkpointed local edits, loud on anything outbound or destructive.
Products that prompt uniformly train users to click "allow all."

### 4.3 One engine, two vocabularies
Everyday mode and Developer mode are the same session with different words —
Project/Documents/checkpoint/Undo/Connector/Playbook vs
repo/files/commit/revert/MCP/skill. Hidden, never removed; one toggle reveals
everything.

This matters more than it looks, because of who installs software: the technical
person evaluates it, the non-technical team uses it. A product that serves only
one of them dies at the boundary. Eavery is the only one that can be handed
sideways across that boundary without switching tools.

### 4.4 No API key — use the AI you already have
Anthropic's ban on third-party subscription OAuth (Jan–Apr 2026) broke the
onboarding of every competitor in this category; the standard OSS answer is now
"get comfortable managing API keys," which for the target user is a wall.

Eavery's answer: **be an ACP client, not a harness.** Drive the user's own,
officially installed, officially authenticated Codex CLI, Claude Code, or Gemini
CLI. Their subscription, their sign-in, no proxying, no second bill.

Three honest caveats, all from `01-landscape.md` §4:
- It is **table stakes, not a moat**: goose already ships the same
  subscription sign-in through its own ACP providers.
- The **Claude route is tolerated, not guaranteed**. The adapter is built on
  the Agent SDK, which Anthropic announced it would bill separately from May
  2026 and then paused. Codex (ChatGPT sign-in) is the primary zero-key engine;
  Claude and Gemini are supported but must never be load-bearing.
- **The target user does not have these CLIs installed.** The path is only
  "no key" if Eavery installs the CLI itself. Codex CLI and `codex-acp` ship
  native binaries, so Eavery downloads them and launches the browser sign-in
  from inside the app; that is the one genuinely terminal-free zero-key path in
  v1. The Claude and Gemini adapters need Node and are labelled as such.

## 5. Wedge: pick one job, be undeniable at it

Horizontal "AI for all office work" is unwinnable against Anthropic's distribution.
Win a beachhead where the work is **repetitive, multi-file, deadline-driven, and
currently done by hand.**

Recommended wedge: **finance and operations reporting**, narrowed for v1 to
the tasks the v1 document Connector can actually do: find-and-replace across
documents, reconciling sheets, summarising folders, extracting PDF tables.
Refreshing a monthly report with charts and decks needs chart-aware `.xlsx`
editing and `.pptx` writing, which v1 does not have. Do not demo what the
tools cannot deliver.

- Multi-source, multi-step, monthly cadence — exactly the shape agents are good at.
- The artifacts are `.xlsx` and `.pptx`, where deterministic tooling beats
  chat outright and the quality gap is obvious in one demo.
- Correctness is checkable, so trust compounds instead of eroding.
- Painful enough that people tolerate v1 rough edges.
- Data-sensitive enough that **local-first and open-source are purchase reasons,
  not ideology.** This is where an open Rust desktop app beats a cloud product on
  the merits rather than on price.

Land there with 5–10 excellent Playbooks. Expand outward through the Playbook
ecosystem, not through Eavery's own roadmap.

## 6. Moat

Honest assessment: the code is not the moat — it is mostly borrowed, and
deliberately so.

1. **The Playbook library** (weak → strong over time). Portable by design, so
   individual playbooks aren't lockin; the *curated, verified, works-out-of-the-box*
   collection is. Compounds with users. This is the main long-term asset.
2. **Trust reputation.** In this category, "has never destroyed anyone's work" is a
   durable brand asset. It is also fragile — one incident spends it entirely.
3. **Local-first + open source.** Structurally uncopyable by Anthropic and OpenAI,
   whose businesses require inference. Buys the regulated verticals — legal,
   healthcare, finance, public sector, EU data residency. Be honest that this
   is only true on the Ollama path: on every other engine, every file the agent
   reads is sent to the model vendor, and the plan card must say so. Local
   models are weakest at exactly the multi-file office work the wedge needs,
   so the regulated-vertical story is a Phase 2 claim, not a v1 one.
4. **Provider neutrality.** Also structurally uncopyable by the model vendors.
5. **Community.** Contributors write Playbooks and Connectors in English, not Rust
   — a far larger contributor pool than a normal OSS project. Design for this
   explicitly: it is the difference between a repo and a movement.

## 7. Roadmap

**Phase 0 — Prove the seam (weeks 1–6).**
Tauri v2 shell. ACP client. Drive embedded goose *and* the user's Claude Code
through the same interface. Git-backed workspace with working Undo.
*Exit test:* swap engines mid-session; the UI doesn't notice.

**Phase 1 — Everyday mode (weeks 6–14).**
Vocabulary projection. Plan gate. Permission model on the irreversibility axis.
Document Connectors (docx/xlsx/pptx/pdf). Zero-key onboarding.
*Exit test:* a non-technical person completes a real task unaided, and undoes it.

**Phase 2 — The wedge (weeks 14–26).**
5–10 finance/ops Playbooks. Playbook authoring in plain English. Connectors for
Drive/SharePoint, email/calendar, Slack/Teams. Audit log. Ollama path.
*Exit test:* a finance team runs their month-end on it, twice, unattended.

**Phase 3 — Ecosystem (26+).**
Playbook registry (`agentskills.io`-compatible, never proprietary). Scheduled and
event-triggered runs (Kiro-style hooks). Team sync of playbooks and policy.
*Exit test:* more playbooks arrive from outside the core team than inside it.

## 8. Monetisation (open core, if desired)

Free and open forever: the app, the engine, single-user everything. Anything that
makes an individual worker productive stays free — that is the growth engine and
the reason contributors show up.

Commercial surface is **the team boundary**, which is genuinely expensive to build
and genuinely valuable to companies:
- Shared Playbook registry with review and approval workflow
- Central policy: which connectors, which models, what leaves the machine
- Audit and compliance export, SSO/SCIM
- Signed/verified Playbooks and Connectors
- Support and certified deployments (the actual near-term revenue in regulated verticals)

Never paywall: model access, connector count, local usage, or undo.

## 9. What would make this fail

- **Onboarding.** If first-run demands an API key, nothing else matters. P0.
- **Chasing Cowork feature-for-feature.** Guaranteed loss. Win on trust,
  neutrality, and locality instead.
- **Building the engine.** Every month spent on an agent loop is a month not spent
  on the differentiators, against a competitor with 500 contributors.
- **Trust incident.** One unrecoverable data loss ends it. Ship undo before scale.
- **Inventing formats.** A proprietary playbook or connector format forfeits ~40
  products' worth of ecosystem for nothing.
- **Staying horizontal.** No wedge means no reference customers, no proof, no
  reason to switch.
- **Building for three months before a real user touches it.** The plan's
  first usability test was at week 13. Sit with three to five finance or ops
  people now and watch them attempt a month-end task with Claude Code or
  Cowork on their own files; that establishes whether the engines can do the
  work at all, which nothing in this repo yet shows.

## 10. Open questions

1. **Is BYO-agent-over-ACP durable?** For Claude, Anthropic has already
   announced (May 2026) and paused (June 2026) a move to separate billing for
   ACP and Agent SDK use; assume it returns. For Codex and Gemini there is no
   such announcement. Ollama and BYO-key are the hedge; do not let any single
   zero-key engine become the only path.
2. **Which wedge?** Finance/ops is the recommendation, but legal review and
   research/consulting are plausible. Decide from access to real users, not from
   analysis — whichever vertical you can get five design partners in this month.
3. **Contribute the Tauri shell upstream to goose, or keep it?** Contributing buys
   credibility and maintenance; keeping it buys a head start. Recommendation:
   contribute — the shell is not the moat, and goodwill with an LF project is worth
   more than three months of lead.
4. **Name and licence.** MIT today. Apache 2.0 is worth considering for the patent
   grant, which matters to enterprise legal review and matches goose and Codex.
