# 09 — Document Connector and Playbooks

## 1. `eavery-docs-mcp`

A standalone binary MCP server (stdio transport) built with the official
`rmcp` crate. Eavery passes it to every engine in `session/new` under the name
`eavery-docs`. It is also usable from any other MCP client, which is a small
open-source gift on its own.

### 1.1 Tools (v1, exact names)

| Tool | Input | Output | Crate |
|---|---|---|---|
| `docx_read_text` | `{path}` | paragraphs with style names and table cells as text, in order | `docx-rs` (`read_docx`) |
| `docx_replace_text` | `{path, replacements:[{find, replace}], match_case?}` | count per replacement; writes in place, preserving runs by operating on `w:t` text nodes via `zip`+`quick-xml` | `zip`, `quick-xml` |
| `docx_append_paragraphs` | `{path, paragraphs:[{text, style?}]}` | ok | `zip`+`quick-xml` (insert before `w:sectPr`) |
| `xlsx_list_sheets` | `{path}` | sheet names and used ranges | `calamine` |
| `xlsx_read_range` | `{path, sheet, range?}` | 2-D array of cell values (typed: number/string/bool/date/empty) | `calamine` |
| `xlsx_write_cells` | `{path, sheet, cells:[{ref, value \| formula}]}` | ok; creates the sheet if missing | `umya-spreadsheet` |
| `xlsx_create` | `{path, sheets:[{name, rows:[[...]]}]}` | ok | `rust_xlsxwriter` |
| `pdf_read_text` | `{path, pages?}` | text per page | `pdf-extract` (falls back to `lopdf` raw strings) |
| `pptx_read_text` | `{path}` | text per slide with slide numbers and notes | `zip`+`quick-xml` on `ppt/slides/slide*.xml` |
| `doc_info` | `{path}` | type, size, page/sheet/slide count, last modified | mixed |

Every write tool: writes to `<path>.eavery-tmp`, re-opens the result with the
reader crate to validate, then renames over the original. On validation failure
it deletes the temp file and returns an error `"The change would have damaged
the file, so nothing was written."`.

Every tool: rejects paths outside the directory passed as `--root` (Eavery
passes the Project root) with a clear error. Follows no symlinks outside root.

### 1.2 Tool descriptions (what the engine reads)

Descriptions must tell the model when to use the tool and what it preserves.
Example for `docx_replace_text`:

> Replace text in a Word document while preserving all formatting, styles,
> headers, footers, and images. Use this instead of writing scripts or
> regenerating the document. Matches are found across formatting runs inside a
> paragraph. Returns how many replacements were made per pattern.

State the limits too, so the model does not guess: "Cannot change images or
tracked changes. Cannot write .pptx files."

### 1.3 Skeleton with rmcp

```rust
use rmcp::{ServerHandler, ServiceExt, model::*, tool, tool_router, tool_handler, handler::server::router::tool::ToolRouter};

#[derive(Clone)]
struct Docs { root: std::path::PathBuf, tool_router: ToolRouter<Self> }

#[tool_router]
impl Docs {
    #[tool(description = "Read the text of a Word document, in order, with paragraph styles.")]
    async fn docx_read_text(&self, Parameters(p): Parameters<PathArg>) -> Result<CallToolResult, rmcp::ErrorData> { /* ... */ }
    // one method per tool
}

#[tool_handler]
impl ServerHandler for Docs {
    fn get_info(&self) -> ServerInfo {
        ServerInfo { instructions: Some("Deterministic tools for Office documents. Prefer these over scripts.".into()), capabilities: ServerCapabilities::builder().enable_tools().build(), ..Default::default() }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(2).map(Into::into).unwrap_or(std::env::current_dir()?); // --root <path>
    let service = Docs::new(root).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

Macro names and module paths change between `rmcp` minor versions. Open
docs.rs for the pinned version and the `examples/servers` folder in the SDK
repository, and adapt. The shape above (a struct, a router of `#[tool]`
methods, `ServerHandler`, stdio transport) is stable.

### 1.4 Tests
- Golden files in `crates/eavery-docs-mcp/tests/fixtures/`: a `.docx` with
  headings, a table, bold runs split mid-word; an `.xlsx` with two sheets,
  formulas, dates; a two-page `.pdf`; a three-slide `.pptx` with notes.
- Each write tool: run, then re-read with the reader tool and assert; also
  assert the zip still contains every original part (`[Content_Types].xml`,
  `word/styles.xml`, media) so formatting survives.
- Path escape attempts (`../`, absolute outside root, symlink) are rejected.
- Run the server over stdio in a test using `rmcp`'s client transport, list
  tools, call `doc_info`.

## 2. Playbooks (Agent Skills)

Eavery does not invent a format. A Playbook is a folder with `SKILL.md`
per https://agentskills.io/specification: YAML frontmatter with required
`name` (1–64 chars, lowercase letters, digits, hyphens, not starting or ending
with a hyphen) and `description` (1–1024 chars), optional `license`,
`compatibility`, `metadata`, `allowed-tools`; then a markdown body. Extra
files (`scripts/`, `references/`, `assets/`) may sit beside it.

### 2.1 Discovery
Search, in this order, and de-duplicate by `name` (first wins):
1. `<project>/.agents/skills/*/SKILL.md`
2. `<project>/.claude/skills/*/SKILL.md` (Claude Code's location; read-only compatibility)
3. `~/.eavery/playbooks/*/SKILL.md`
4. Bundled: `playbooks/*/SKILL.md` in the app resources.

Parse frontmatter with `serde_yaml` into `{name, description, ...}`; invalid
Playbooks are listed with an error and not injected.

### 2.2 Injection
Engines that read skills natively (Claude Code reads `.claude/skills`; goose
and Codex read `.agents/skills` and `AGENTS.md`) find project Playbooks on
their own. For Playbooks from the library and bundle, Eavery injects the
**name and description only** into the plan prompt's `{{playbooks}}` block,
with the absolute path to the `SKILL.md`, and instructs: "If a playbook
matches, read its SKILL.md and follow it." This mirrors the spec's progressive
disclosure and keeps prompts small. Eavery does not copy Playbooks into the
Project folder.

### 2.3 Bundled Playbooks for v1 (five, wedge-aligned)
Written in plain English, each under 150 lines, each tested once manually
with two engines:

1. `monthly-report-refresh` — update a Word report from a spreadsheet's new
   month column; list every number changed.
2. `spreadsheet-reconcile` — compare two sheets on a key column; produce a
   third sheet of differences; never modify the sources.
3. `find-and-replace-across-documents` — the M5 exit-test task, generalised.
4. `summarise-folder` — one-page summary of every document in a folder into
   `SUMMARY.md` (read-only on sources).
5. `pdf-to-spreadsheet-table` — extract tables from PDFs into an `.xlsx`,
   with a "check these cells" list.

Each Playbook's body states: which `eavery-docs` tools to use, what not to
do (no email, no deletion), and what the final summary must contain.

### 2.4 UI
Settings → Playbooks lists name, description, source (Project / Library /
Built-in), and an "Open folder" button. Composer has a "Use a Playbook" menu
that inserts "Use the {name} playbook: " into the request. No editor in v1.
