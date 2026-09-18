// The Activity pane (`07-ui-vocabulary.md` §3). Everyday: a short trail of
// one-line tool events, and the history with Undo. Developer: the raw tool
// calls, permission decisions with who made them, plan entries, phase
// changes, and the Diagnostics tab.

import { useState } from "react";
import { useStore } from "../store";
import { blocksFor, type Block } from "../transcript";
import { useShown, useT } from "../vocab/useT";
import { Checkpoints } from "./Checkpoints";
import { Diagnostics } from "./Diagnostics";
import { ToolCallRow } from "./ToolCallRow";
import { permissionTitle } from "./PermissionDialog";
import { decisionWords } from "./Transcript";

type Tab = "activity" | "history" | "diagnostics";

/** How many trail lines Everyday mode keeps on screen. */
const TRAIL = 30;

function Trail({ blocks }: { blocks: Block[] }) {
  const t = useT();
  const { settings } = useStore();
  const developer = settings.mode === "developer";

  const rows = blocks.filter(
    (block) =>
      block.kind === "tool" ||
      block.kind === "permission" ||
      (developer && (block.kind === "plan_entries" || block.kind === "phase" || block.kind === "engine_status")),
  );
  const shownRows = developer ? rows : rows.slice(-TRAIL);

  if (shownRows.length === 0) return <p className="muted">{t("noActivityYet")}</p>;

  return (
    <ul className="trail">
      {shownRows.map((block) => (
        <li key={block.key}>
          {block.kind === "tool" ? (
            <ToolCallRow call={block.call} />
          ) : block.kind === "permission" ? (
            <span className="tool">
              <span className="tool-line">{permissionTitle(t, block.request)}</span>
              {block.decision && block.by ? (
                <span className="chip">{decisionWords(t, block.decision, block.by)}</span>
              ) : null}
            </span>
          ) : block.kind === "plan_entries" ? (
            <span className="tool">
              <span className="tool-line">{t("planEntries")}</span>
              <span className="chip">{block.entries.length}</span>
            </span>
          ) : block.kind === "phase" ? (
            <span className="tool muted">{t("turnPhase", { phase: block.phase })}</span>
          ) : block.kind === "engine_status" ? (
            <span className="tool muted">
              {t("engineStatusLine", { engine: block.engineId, state: block.status.state })}
            </span>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

export function Activity() {
  const { transcript, turns } = useStore();
  const t = useT();
  const shown = useShown();
  const [tab, setTab] = useState<Tab>("history");
  const blocks = blocksFor(transcript, turns);
  const diagnosticsTab = shown("tabDiagnostics");
  const current: Tab = tab === "diagnostics" && !diagnosticsTab ? "history" : tab;

  return (
    <div className="activity">
      <div className="tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={current === "activity"}
          onClick={() => setTab("activity")}
        >
          {t("tabActivity")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={current === "history"}
          onClick={() => setTab("history")}
        >
          {t("tabHistory")}
        </button>
        {diagnosticsTab ? (
          <button
            type="button"
            role="tab"
            aria-selected={current === "diagnostics"}
            onClick={() => setTab("diagnostics")}
          >
            {t("tabDiagnostics")}
          </button>
        ) : null}
      </div>
      <div className="tab-body" role="tabpanel">
        {current === "activity" ? <Trail blocks={blocks} /> : null}
        {current === "history" ? <Checkpoints /> : null}
        {current === "diagnostics" ? <Diagnostics /> : null}
      </div>
    </div>
  );
}
