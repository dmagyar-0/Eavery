# 08 — Onboarding, Engine Detection, Packaging

First run is a P0 engineering problem (`docs/02-building-blocks.md` §3). This
document specifies it precisely.

## 1. First-run flow

```
Welcome ─▶ Detecting assistants… ─▶ [found ≥1 zero-key engine] ─▶ Pick one ─▶ Health check ─▶ Open a folder ─▶ Done
                                  └▶ [found none] ─▶ Choose: "I have ChatGPT" | "I have Claude or Gemini" | "I have an API key" | "Keep everything on this computer"
                                                       │                 │                              │                          │
                                                       ▼                 ▼                              ▼                          ▼
                                            Download Codex CLI   Node present? ─ yes ▶ npx adapter    Download goose ─▶ paste key    Download goose ─▶ detect Ollama ─▶ pick model
                                            + codex-acp          │                    + sign-in        ─▶ health check                 ─▶ health check
                                            ─▶ "Sign in" opens   └─ no ▶ NeedsNode copy: install
                                               browser (codex login)     Node, or use ChatGPT instead
                                            ─▶ health check
```

Rules:
- Detection runs in the background while the welcome screen shows; the user
  never waits on a blank screen.
- **The ChatGPT path never shows a terminal command.** Eavery downloads
  Codex CLI and `codex-acp` (§4), then spawns `codex login`, which opens the
  browser; Eavery waits for the process to exit and re-runs the health check.
  This is the one path that fulfils "no API key" for a person who has never
  installed a CLI, and it is verified in S0 spike 1.
- The Claude and Gemini paths need Node for their adapters. If Node is
  present, Eavery runs the adapter via `npx` and shows the sign-in command
  (the one string shown verbatim). If not, the `NeedsNode` copy offers the
  ChatGPT path instead. Bundling Node is in `BACKLOG.md`.
- Never ask for an API key on the first screen. The key path is the third
  option and is labelled honestly: "Pay per use with your own key".
- Every path ends in the same health check and the same "Open a folder" step.
- The whole flow is re-enterable from Settings → Assistants.

## 2. Engine detection

For each `EngineSpec`, resolve the executable:

1. `settings.engine_paths[id]` if set.
2. `which`-style search over the fixed PATH (see `02-challenges.md` C5;
   use the `which` crate after applying `fix-path-env`).
3. Well-known locations:

| Platform | Locations checked |
|---|---|
| macOS | `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.claude/local`, `~/.volta/bin`, `~/.nvm/versions/node/*/bin`, `~/.cargo/bin`, `~/.bun/bin` |
| Linux | `/usr/local/bin`, `~/.local/bin`, `~/.claude/local`, `~/.nvm/versions/node/*/bin`, `~/.volta/bin`, `~/.cargo/bin`, `/snap/bin` |
| Windows | `%LOCALAPPDATA%\Programs\*`, `%APPDATA%\npm`, `%USERPROFILE%\.claude\local`, `%USERPROFILE%\.cargo\bin`, `%ProgramFiles%\nodejs`, `%LOCALAPPDATA%\Microsoft\WinGet\Links` |

For npm-distributed adapters (`claude-agent-acp`, `codex-acp` via npm), check
for the global binary first; if absent but `node`/`npx` is present, launch via
`npx -y <package>` and warn that first start may take a minute (npm download).
Also check whether the underlying CLI exists (`claude`, `codex`, `gemini`)
because the adapter is useless without a login.

Detection results are cached for the session and refreshed by "Check again".

## 3. Sign-in instructions (shown verbatim)

| Engine | Instruction |
|---|---|
| claude | "Install Claude Code from https://claude.com/claude-code, open Terminal, run `claude` once and sign in. Then install the bridge: `npm install -g @agentclientprotocol/claude-agent-acp`." |
| codex | Handled inside Eavery: download Codex CLI and `codex-acp` (§4), then a "Sign in with ChatGPT" button that runs `codex login`. Fallback text if the download is blocked (offline, proxy): "Install Codex CLI from https://github.com/openai/codex/releases, run `codex login`, then check again." |
| gemini | "Install Gemini CLI (`npm install -g @google/gemini-cli`), run `gemini` once and sign in with Google." |
| goose (key) | Handled inside Eavery: download, paste key. |
| goose-local | "Install Ollama from https://ollama.com and pull a model, for example `ollama pull qwen3:8b`." (Do not hardcode a model that may not exist; read the list from Ollama and offer the installed ones.) |

Keep these strings in one file, `crates/eavery-engines/src/instructions.rs`,
so they can be updated without touching UI code.

## 4. Engine downloads (goose, Codex CLI, codex-acp)

One mechanism, three sources, behind one `EngineSource` enum (`Bundled`,
`Download { url_pattern, sha256 }`, `UserInstalled`):

| Binary | Source | Licence |
|---|---|---|
| goose | GitHub releases of `aaif-goose/goose` | Apache 2.0 |
| Codex CLI | GitHub releases of `openai/codex` (`codex-<triple>` archives) | Apache 2.0 |
| codex-acp | npm `@agentclientprotocol/codex-acp` platform packages (tarballs fetched from the npm registry by URL, no npm client needed), or its GitHub releases | Apache 2.0 |

- Record the exact release URL pattern and SHA-256 checksums for each pinned
  version in `crates/eavery-engines/src/releases.rs`. Pin one version of
  each; bump deliberately. Codex CLI's own auto-update is disabled by
  launching it with the downloaded binary path and never touching `~/.codex`.
- Download to `<data_dir>/engines/<id>/<version>/`, verify checksum, set
  executable bit, run `<bin> --version` to confirm. Emit progress events for
  the `Installing` status copy in `07` §5.
- On macOS, the downloaded binary will be quarantined; run
  `xattr -d com.apple.quarantine <path>` or the user gets a Gatekeeper
  dialog. Test this on a clean Mac.
- Keys entered by the user go to the OS keychain via the `keyring` crate
  under service `dev.eavery.Eavery`, account `<provider>`. They are passed to
  the goose child as env vars and never written to disk by Eavery.
- Sign-in for Codex: spawn `<codex> login` with the same PATH fix, with
  stdout/stderr captured; it opens the system browser itself. Wait for exit
  (10 minute timeout), then run the health check. Eavery never reads the
  resulting `~/.codex/auth.json`.
- Offline installers (regulated environments) can bundle any of these as a
  Tauri sidecar instead (`bundle.externalBin`); that is the `Bundled` variant
  of `EngineSource`.

## 5. Health check UX

Run `run_health_check` (`04-acp-engines.md` §9) with a progress line per
step: "Starting…", "Connected", "Signed in", "Answered". Failure copy per
`07-ui-vocabulary.md` §5. A health check that passes writes
`EngineStatus::Ready` with timestamp; the Project screen shows "Ready" and
does not re-check for 10 minutes.

## 6. Packaging

- `tauri.conf.json`: `identifier` `dev.eavery.Eavery`; `bundle.targets`
  `["dmg","app","msi","nsis","appimage","deb"]`; `plugins.updater` with a
  public key generated by `tauri signer generate` (private key in CI secrets
  only); `bundle.externalBin` includes `eavery-docs-mcp` (the MCP server binary
  is always bundled; goose is downloaded).
- macOS: sign and notarise in CI (Apple Developer ID). Until credentials
  exist, ship unsigned builds labelled "dev" and document the
  right-click → Open workaround in the README. Do not silently ship unsigned.
- Windows: MSI via WiX (Tauri default). Code signing later; document the
  SmartScreen prompt.
- Linux: AppImage and deb. AppImage must not be sandboxed (needs to spawn
  user binaries).
- Release workflow: tag `v*` → GitHub Actions matrix build → attach installers
  and `latest.json` for the updater.
- App size target: under 30 MB per platform without goose.

## 7. Durability across restart

- On startup, load Projects and the last open Project; render transcript from
  `list_events`.
- Re-spawn the engine lazily on the first new request, not at startup.
- If the engine reported `loadSession: true` and `session.engine_session_id`
  exists, call `session/load`; the replayed `user_message_chunk` /
  `agent_message_chunk` updates are ignored for the transcript (already in
  SQLite) but the session id is reused. Otherwise open a new session and
  prepend a summary to the first prompt as "Context from the previous
  conversation". Build it from **messages, not chunks**: coalesce consecutive
  `AgentText` events of one turn into one message, take the last 10 user
  requests and agent messages, and cap at 2,000 characters. (`AgentText` is a
  streamed chunk; "the last 20 events" would be about 20 tokens.)
- If the app was closed mid-turn, the turn is marked `Failed` with next action
  "Eavery was closed while working. Your files are protected; check History."
  and the post-checkpoint is taken at next open.
