//! Sign-in instructions, shown verbatim (`docs/plan/08-onboarding-packaging.md` §3).
//!
//! They live in one file so the wording can be updated without touching UI
//! code, and so the one command a user is ever asked to type is visible in a
//! single place when an engine changes how it signs people in.

pub const CLAUDE: &str = "Install Claude Code from https://claude.com/claude-code, open Terminal, \
     run `claude` once and sign in. Then install the bridge: \
     `npm install -g @agentclientprotocol/claude-agent-acp`.";

/// Codex is handled inside Eavery: Eavery downloads Codex CLI and `codex-acp`
/// and runs `codex login` itself (M7-T02b, M7-T02c). This is the fallback text
/// for when the download cannot happen — offline, or behind a proxy.
pub const CODEX: &str = "Install Codex CLI from https://github.com/openai/codex/releases, \
     run `codex login`, then check again.";

pub const GEMINI: &str = "Install Gemini CLI (`npm install -g @google/gemini-cli`), \
     run `gemini` once and sign in with Google.";

/// goose is downloaded by Eavery and takes a pasted key (M7-T02, M7-T03). The
/// text below is what is shown when the download is unavailable.
pub const GOOSE: &str = "Install goose from https://github.com/aaif-goose/goose/releases, \
     then paste an API key for your provider in Settings.";

/// Deliberately names no model: the list comes from Ollama itself, because a
/// model named here may not be one the user has pulled.
pub const GOOSE_LOCAL: &str = "Install Ollama from https://ollama.com and pull a model, \
     for example `ollama pull qwen3:8b`.";

pub const FAKE: &str = "The fake engine ships with Eavery's own tests. \
     Build the workspace to get it.";

/// The one string the `NeedsNode` screen shows (`07-ui-vocabulary.md` §5 holds
/// the sentence around it).
pub const NEEDS_NODE: &str = "Install Node.js from https://nodejs.org, \
     or use ChatGPT (Codex) instead, which Eavery can set up for you.";
