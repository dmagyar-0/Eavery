// One tool call. Everyday: one line with no tool names; Developer: the raw
// kind, status, locations and risk (`07-ui-vocabulary.md` §3).

import { memo } from "react";
import type { ToolCallView } from "../types";
import { fileName } from "../transcript";
import { useShown, useT, type T } from "../vocab/useT";
import { useStore } from "../store";

/** The one-line rendering of a call, in the current mode. Empty when hidden. */
export function describeCall(t: T, call: ToolCallView): string {
  const file = call.locations[0] ? fileName(call.locations[0]) : call.title;
  switch (call.kind) {
    case "read":
      return t("toolRead", { file });
    case "edit":
      return t("toolEdit", { file });
    case "delete":
      return t("toolDelete", { file });
    case "move":
      return t("toolMove", { file });
    case "execute":
      return t("toolExecute", { title: call.title });
    case "fetch":
      return t("toolFetch", { title: call.title });
    case "search":
      return t("toolSearch", { title: call.title });
    case "think":
      return t("toolThink", { title: call.title });
    default:
      return t("toolOther", { kind: call.kind, title: call.title });
  }
}

export const ToolCallRow = memo(function ToolCallRow({ call }: { call: ToolCallView }) {
  const t = useT();
  const shown = useShown();
  const { settings } = useStore();
  const developer = settings.mode === "developer";

  if (call.kind === "think" && !shown("toolThink")) return null;

  return (
    <div className={`tool tool-${call.status}`} data-kind={call.kind}>
      <span className="tool-line">{describeCall(t, call)}</span>
      {developer ? (
        <span className="tool-meta">
          <span className="chip">{t("toolStatus", { status: call.status })}</span>
          <span className="chip">{t("toolRisk", { risk: call.risk })}</span>
          {call.locations.length > 0 ? (
            <span className="tool-locations">
              {t("toolLocations", { locations: call.locations.join(", ") })}
            </span>
          ) : null}
        </span>
      ) : null}
      {call.diff_summary ? <span className="tool-diff">{call.diff_summary}</span> : null}
    </div>
  );
});
