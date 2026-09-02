# Plan changelog

Record here every place where implementation reality differed from the plan
documents, and every locked decision that had to change, with the reason.
Newest first.

| Date | Task | What differed | What was done |
|---|---|---|---|
| 2026-09-02 | M0-T01 | Several crate versions in `03-architecture.md` §2 do not exist or are stale on crates.io | Pinned what crates.io serves: `git2` 0.21 (plan said 0.20), `rusqlite` 0.40 (0.32), `rmcp` 3.2 (0.8), `ts-rs` 12 (10), `which` 8 (7), `keyring` 4 (3), `calamine` 0.36 (0.26), `umya-spreadsheet` 3.1 (2), `rust_xlsxwriter` 0.99 (0.80), `pdf-extract` 0.12 (0.8), `lopdf` 0.44 (0.34), `quick-xml` 0.42 (0.37), `zip` 8 (2). `agent-client-protocol` 2.0.0 and `agent-client-protocol-schema` 1.7.0 are as planned. `fix-path-env` is not in the workspace manifest yet; it arrives with M1-T02. |
| 2026-09-01 | (review) | `REVIEW-2026-09.md` found spec bugs and inconsistencies across the plan | Applied: D15 (reads allowed outside Project), D16 (checkpoint before restore), `PlanJson` wire struct, `Engine` trait on `&self`, `plan_mode_hint` + plan-mode-exit refusal, `@agentclientprotocol/codex-acp` (Zed package deprecated), `agent-client-protocol-schema = "1"`, Codex CLI + codex-acp download and in-app `codex login`, shallow health check by default, exclude list additions, cloud-placeholder guard, C11–C13 added (prompt injection, Journal growth, concurrency), M5/M6 exit tests swapped, S0 spikes added, vocab lint restricted to strings, `opener` plugin, Ctrl+Z scoping, message-coalesced resume summary. |
