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
| Onboarding | excellent | poor (API keys) | N/A | **BYO-agent: no key** |
| Undo | limited | limited | git (visible) | **git (invisible)** |
| Supervision | permissions | permissions | diffs | **plain-English plan gate** |
| Runtime | cloud+desktop | Electron/Python | CLI | **Rust/Tauri, ~5MB** |

**One-line positioning:** *the open, local-first agent desktop that works with the
AI you already pay for.*

## 4. The four differentiators

Everything else is table stakes. These are the bets.

### 4.1 No API key — use the AI you already have
Anthropic's ban on third-party subscription OAuth (Jan–Apr 2026) broke the
onboarding of every competitor in this category; the standard OSS answer is now
"get comfortable managing API keys," which for the target user is a wall.

Eavery's answer: **be an ACP client, not a harness.** Drive the user's own,
officially installed, officially authenticated Claude Code or Codex CLI. Their
subscription, their ToU, no proxying, no grey zone, no second bill.

This turns the category's biggest constraint into the best first-run experience in
the category. It is also the hardest thing for a closed vendor to copy, because
Anthropic will not ship "works great with ChatGPT."

### 4.2 Invisible git — undo anything
Every project is a git repo the user never sees as git. Automatic checkpoint
before every action; one Undo button; nothing ever truly lost.

An agent with write access to real work is frightening until it is *provably*
reversible. This is the cheapest trust available — the hard engineering was done
by someone else in 2005 — and it is the precondition for anyone letting an agent
near a document that matters.

### 4.3 The plan gate — supervision without literacy
Adapted from Kiro's spec-driven pattern. Plan in plain English → review and edit →
execute → digest. A non-technical user cannot audit a diff, but they can absolutely
read "I'll pull the three regional sheets, reconcile against the ledger, and draft
a summary — I won't email anything."

Critically, permission friction should track **irreversibility**, not action type:
near-silent on checkpointed local edits, loud on anything outbound or destructive.
Products that prompt uniformly train users to click "allow all."

### 4.4 One engine, two vocabularies
Everyday mode and Developer mode are the same session with different words —
Project/Documents/checkpoint/Undo/Connector/Playbook vs
repo/files/commit/revert/MCP/skill. Hidden, never removed; one toggle reveals
everything.

This matters more than it looks, because of who installs software: the technical
person evaluates it, the non-technical team uses it. A product that serves only
one of them dies at the boundary. Eavery is the only one that can be handed
sideways across that boundary without switching tools.

## 5. Wedge: pick one job, be undeniable at it

Horizontal "AI for all office work" is unwinnable against Anthropic's distribution.
Win a beachhead where the work is **repetitive, multi-file, deadline-driven, and
currently done by hand.**

Recommended wedge: **finance and operations reporting.**

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
   healthcare, finance, public sector, EU data residency.
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

## 10. Open questions

1. **Is BYO-agent-over-ACP durable?** It is legitimate today — the user's own
   client authenticates itself. Worth watching whether vendors move against
   ACP-driving too. Ollama and BYO-key are the hedge; do not let it become the
   only path.
2. **Which wedge?** Finance/ops is the recommendation, but legal review and
   research/consulting are plausible. Decide from access to real users, not from
   analysis — whichever vertical you can get five design partners in this month.
3. **Contribute the Tauri shell upstream to goose, or keep it?** Contributing buys
   credibility and maintenance; keeping it buys a head start. Recommendation:
   contribute — the shell is not the moat, and goodwill with an LF project is worth
   more than three months of lead.
4. **Name and licence.** MIT today. Apache 2.0 is worth considering for the patent
   grant, which matters to enterprise legal review and matches goose and Codex.
