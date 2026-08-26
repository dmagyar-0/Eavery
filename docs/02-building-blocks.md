# Building Blocks: Reference Architecture

How Eavery is assembled, and which parts are bought, borrowed, or built.

## 0. The one-paragraph version

Eavery is a **Tauri v2 desktop shell** over a **Rust core** that acts as an **ACP
client**. The agent engine is swappable: embedded goose by default, or the user's
own Claude Code / Codex CLI. Capabilities arrive as **MCP servers** ("Connectors")
and **Agent Skills** ("Playbooks"). Every run is wrapped in a **git-backed
workspace** the user never sees as git, and gated by a **plan-approve-execute**
loop borrowed from Kiro's spec-driven pattern. The Developer/Everyday split is a
**presentation layer**, not two engines.

## 1. Layer map

```
┌──────────────────────────────────────────────────────────────┐
│  L6  Shell            Tauri v2 · webview UI · tray · updater │
├──────────────────────────────────────────────────────────────┤
│  L5  Vocabulary       Everyday ⇄ Developer mode projection   │
├──────────────────────────────────────────────────────────────┤
│  L4  Trust            plan gate · permissions · undo · audit │
├──────────────────────────────────────────────────────────────┤
│  L3  Session          workspace · git journal · checkpoints  │
├──────────────────────────────────────────────────────────────┤
│  L2  Agent            ACP client → goose | Claude Code | Codex│
├──────────────────────────────────────────────────────────────┤
│  L1  Capability       MCP servers · Agent Skills · sandbox   │
├──────────────────────────────────────────────────────────────┤
│  L0  Model            Anthropic · OpenAI · Google · Ollama   │
└──────────────────────────────────────────────────────────────┘
```

Build effort concentrates in **L3–L5**. That is deliberate: L0–L2 are commodity
and standardising fast, L6 is a solved problem. The defensible work is the trust
layer and the vocabulary layer, and almost nobody else is doing it.

## L0 — Model access

| Path | Mechanism | Notes |
|---|---|---|
| Bring-your-own-agent | ACP → user's Claude Code / Codex CLI | **No API key.** Their subscription, their ToU, their auth. Best onboarding. |
| Bring-your-own-key | Anthropic, OpenAI, Google, OpenRouter | Usage-based. Required for Anthropic (see landscape §4). |
| Local | Ollama | Fully offline. The regulated-industry and privacy story. |

Never make one provider load-bearing. Provider policy changed twice in 2026 and
took several products with it.

## L1 — Capability

**MCP servers = "Connectors."** Buy, don't build. 70+ exist in goose's ecosystem
alone; Eigent ships 200+. Eavery curates and packages, it does not author.
Priority for office work: filesystem, browser, email/calendar, Slack/Teams,
Drive/SharePoint, CRM, database read-only.

**Agent Skills = "Playbooks."** A folder with `SKILL.md` (YAML frontmatter + a
markdown body). Now supported by ~40 products. This is the single most important
building block for the everyday-worker thesis:

- It is how you deliver **domain competence without code** — "close the month-end
  books", "draft the QBR deck from these three sources", "reconcile this invoice
  batch".
- It is **authored in plain English**, so a finance lead can write one.
- It is **portable**, so Eavery's playbooks work in Claude and Codex and vice
  versa. Do not invent a proprietary format. Being a good citizen of `agentskills.io`
  is cheaper than building a marketplace from zero.
- It gives you a **community flywheel** that is genuinely open-source-shaped:
  contributors donate playbooks, not Rust.

**Document tooling.** Office work is `.docx`, `.xlsx`, `.pptx`, `.pdf`. These
need real OOXML manipulation, not "here's some markdown." Deterministic Rust
crates behind MCP tools beat asking the model to emit valid XML.

**Sandbox.** Do not invent this. Codex CLI is Apache 2.0 and has already solved it
per-platform: Seatbelt (macOS), Landlock + seccomp (Linux), restricted tokens
(Windows). Claude Code's model — `deny` → `ask` → `allow` rule ordering with
gitignore-style path patterns — is the right permission grammar to copy.

## L2 — Agent engine (swappable)

**ACP is the seam.** One protocol, many engines. This is the most important
architectural decision in the project and it should be made on day one, because
retrofitting it later is expensive.

- **Default:** embedded goose (`goosed`, axum, REST + WebSocket). Rust, Apache 2.0,
  Linux Foundation governance, ACP-native since 2.0.
- **Also supported:** the user's own Claude Code, Codex CLI, Gemini CLI.
- **Consequence:** Eavery is never blocked on one vendor's roadmap or policy, and
  every improvement upstream lands in Eavery for free.

**On forking goose vs. depending on it:** depend on it, contribute upstream.
goose has an open discussion (#7332) about migrating its desktop from Electron to
Tauri v2 — the exact work Eavery needs. Doing that work *in the open* buys
credibility, review, and maintenance help that a fork forfeits. Fork only if
upstream rejects a change that is load-bearing for Eavery.

## L3 — Session and the git journal

**The insight: git is the best undo system ever built, and office workers should
never know it's there.**

Every Eavery workspace is a git repo. The user sees:

| Under the hood | What the user sees |
|---|---|
| repo | Project |
| commit before each agent action | automatic checkpoint |
| `git diff` | "here's what changed" — as tracked changes, not a patch |
| `git revert` / reset to checkpoint | **Undo** |
| branch | "try it a different way" |
| reflog | complete history, nothing ever truly lost |

This is a genuine advantage over Cowork and over every "agent edits your files"
product. An agent that can rewrite your quarterly report is terrifying **unless
every single step is reversible with one button.** Reversibility is what converts
"scary demo" into "daily tool", and it is the cheapest trust you will ever buy —
the hard part is already written and battle-tested.

Sessions must also be **durable** — survive app restart and machine sleep. This is
the one idea worth taking from LangGraph (checkpointing, durable execution), and
it should be implemented natively in Rust, not by importing Python.

## L4 — Trust: the plan gate

Borrowed from Kiro's spec-driven pattern, and the answer to "how does a
non-technical person supervise an agent?"

```
Request → PLAN (plain English, reviewable) → approve/edit → EXECUTE → DIGEST
```

- **Plan.** Before touching anything: what it will do, which files, which
  connectors, what it will send outside the machine, what it cannot undo. In the
  user's language. Editable before it runs.
- **Permissions.** `deny → ask → allow`, per Claude Code's ordering. Surfaced as
  plain sentences: "Eavery wants to send this file to Slack. Allow once / always /
  never."
- **Irreversibility is the axis that matters.** Editing a local file is cheap —
  it's checkpointed. *Sending an email* is not. The permission UI should be
  near-silent on reversible actions and loud on outbound and destructive ones.
  Most products get this exactly backwards and prompt-fatigue their users into
  clicking "allow all".
- **Digest.** After the run: what changed, what was sent, one-click undo.
- **Audit log.** Append-only. Needed the day a company with a compliance team
  evaluates Eavery.

## L5 — The vocabulary layer (Everyday ⇄ Developer)

Your "no developer mode that doesn't show code" is right, and it is worth being
precise about *why* it works: **a coding agent is already a general computer
agent.** Read files, run tools, edit, verify, iterate — that loop is
domain-neutral. What makes it feel developer-only is entirely **vocabulary and
chrome**.

So this is a projection over one engine, not a second product:

| Engine concept | Developer mode | Everyday mode |
|---|---|---|
| repository | repo | Project |
| file tree | files | Documents |
| diff | unified diff | tracked changes / before-after |
| commit | commit | checkpoint |
| revert | `git revert` | Undo |
| terminal output | live stream | hidden; "Working on it…" + activity trail |
| MCP server | MCP server | Connector |
| Agent Skill | skill | Playbook |
| model/context | model picker, tokens | hidden; "Fast" / "Careful" |
| error trace | stack trace | "That didn't work. Here's what I'll try instead." |

**Design rules for Everyday mode:**

1. **Hidden, not removed.** One toggle reveals everything. This preserves the
   escape hatch, keeps a single codebase, and means the power user and the finance
   analyst can use the same session — which matters enormously for adoption inside
   a company, because the person who installs it is not the person who needs it.
2. **Artifacts, not process.** Show the *document*, not the operation on it.
3. **Never surface an error as an error.** Surface it as a next action.
4. **The code is still there.** If the agent writes a Python script to reconcile a
   spreadsheet, that is fine and good — the user simply gets the reconciled
   spreadsheet. Hiding code is not the same as not using it; the code is what makes
   the results *correct and repeatable* instead of hallucinated.

## L6 — Shell

**Tauri v2.** Stable, production-ready (v2.10.1, March 2026), ~5MB bundles against
Electron's ~100MB+, Rust backend, native webview, built-in updater and IPC
permission system. Measured Rust agent runtimes use ~5x less memory than Python
equivalents (1.1GB vs 5.1GB peak) — for an always-on desktop app, that is the
difference between a tool people leave running and one they quit.

Dioxus is the alternative (single Rust component model, better mobile path) but is
younger. Tauri is the lower-risk call for v1, and it's where goose is heading too.

## 2. Build / borrow / buy

| Component | Decision | Why |
|---|---|---|
| Agent loop | **Borrow** — goose | Rust, Apache 2.0, LF-governed, ACP-native |
| Engine interop | **Borrow** — ACP | Makes the engine swappable; 25+ agents |
| Tools | **Buy** — MCP ecosystem | 70–200+ servers already exist |
| Domain knowledge | **Borrow** — Agent Skills | Open spec, ~40 products, community-authorable |
| Sandbox | **Borrow** — Codex patterns | Apache 2.0, already platform-correct |
| Desktop shell | **Borrow** — Tauri v2 | Mature; upstream goose is moving there |
| Provider layer | **Borrow** — goose (fallback: Rig) | Already covers 15+ |
| **Git journal / undo** | **Build** | The trust differentiator |
| **Plan gate** | **Build** | How non-technical users supervise agents |
| **Vocabulary layer** | **Build** | The actual product |
| **Onboarding** | **Build** | Where every OSS competitor is weakest |

The pattern: **borrow the entire engine, build the entire experience.** Roughly
80% of the differentiated work sits in L3–L5.

## 3. Risks to design against

1. **Provider policy risk.** Demonstrated twice in 2026. Mitigation: ACP
   bring-your-own-agent + BYO-key + Ollama, all shipped day one.
2. **The API-key cliff.** An office worker will not create an API key. If BYO-agent
   isn't ready at launch, adoption stalls regardless of product quality. Treat
   first-run as a P0 engineering problem, not a polish task.
3. **Upstream churn.** goose 2.0 changed its primary interface. Pin versions,
   contribute upstream, keep the ACP boundary clean so an engine swap is survivable.
4. **Trust is the product.** One unrecoverable data loss ends the project. The git
   journal is not a feature; it is the licence to operate on someone's real files.
5. **A crowded field.** Eigent, Open Cowork, OpenWorker, Hermes are already here,
   and Cowork itself is GA with web and mobile. Being "open-source Cowork" is not a
   position. See `03-vision.md`.
