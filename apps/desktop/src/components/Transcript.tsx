// The conversation: every `CoreEvent`, as rows (`07-ui-vocabulary.md` §3).
//
// The rows come from `blocksFor`, keyed by the `seq` of the event that
// started each one, so a streamed chunk changes one row and a re-render
// touches one row. Developer mode shows everything; Everyday mode hides the
// thoughts and the plumbing, which is what "hidden, not removed" means.

import { memo, useEffect, useRef } from "react";
import { useStore } from "../store";
import { blocksFor, type Block } from "../transcript";
import { useShown, useT, type T } from "../vocab/useT";
import { Digest } from "./Digest";
import { PlanCard } from "./PlanCard";
import { ToolCallRow } from "./ToolCallRow";
import { permissionTitle } from "./PermissionDialog";
import type { DecidedBy, Decision } from "../types";

export function decisionWords(t: T, decision: Decision, by: DecidedBy): string {
  const decided = t(`decision_${decision}`);
  const actor = t(`by_${by}`);
  return t("permissionDecided", { decision: decided, by: actor });
}

const Row = memo(function Row({ block }: { block: Block }) {
  const t = useT();
  const shown = useShown();
  const { settings, checkpoints } = useStore();
  const developer = settings.mode === "developer";

  switch (block.kind) {
    case "request":
      return (
        <div className="row row-request">
          <span className="who">{t("you")}</span>
          <p>{block.text}</p>
        </div>
      );
    case "text":
      return (
        <div className="row row-text">
          <span className="who">{t("assistant")}</span>
          <p className="prose">{block.text}</p>
        </div>
      );
    case "thought":
      if (!shown("thought")) return null;
      return (
        <div className="row row-thought">
          <span className="who">{t("thought")}</span>
          <p className="prose muted">{block.text}</p>
        </div>
      );
    case "tool":
      return <ToolCallRow call={block.call} />;
    case "plan_entries":
      return (
        <div className="row row-plan-entries">
          <span className="who">{t("planEntries")}</span>
          <ol>
            {block.entries.map((entry, index) => (
              <li key={index} data-status={entry.status ?? ""}>
                {entry.content}
                {developer && entry.status ? <span className="chip">{entry.status}</span> : null}
              </li>
            ))}
          </ol>
        </div>
      );
    case "permission":
      return (
        <div className="row row-permission">
          <span className="line">{t("permissionAsked", { what: permissionTitle(t, block.request) })}</span>
          {block.decision && block.by ? (
            <span className="chip">{decisionWords(t, block.decision, block.by)}</span>
          ) : null}
        </div>
      );
    case "plan":
      return (
        <div className="row row-plan">
          <PlanCard plan={block.plan} vendor={block.vendor} turnId={block.turnId} />
        </div>
      );
    case "phase":
      if (!shown("turnPhase")) return null;
      return <div className="row row-note">{t("turnPhase", { phase: block.phase })}</div>;
    case "checkpoint":
      return (
        <div className="row row-note">
          {t("checkpointTaken", { label: block.checkpoint.label })}
        </div>
      );
    case "restored": {
      const label = checkpoints.find((cp) => cp.id === block.to)?.label ?? block.to.slice(0, 8);
      return (
        <div className="row row-note">
          {t("restoredTo", { label, to: block.to })}
          {block.skippedLocked.length > 0 ? (
            <span className="warn">
              {t("lockedFiles", { files: block.skippedLocked.join(", ") })}
            </span>
          ) : null}
        </div>
      );
    }
    case "finished":
      return (
        <div className="row row-finished">
          {block.digest ? (
            <Digest digest={block.digest} />
          ) : (
            <span className="muted">
              {block.stopReason === "cancelled"
                ? t("turnStopped")
                : block.stopReason === "plan_rejected"
                  ? t("turnPlanRejected")
                  : block.stopReason === "failed" || block.stopReason === "crashed"
                    ? t("turnFailed")
                    : t("turnFinished", { reason: block.stopReason })}
            </span>
          )}
        </div>
      );
    case "engine_status":
      if (!shown("engineStatusLine")) return null;
      return (
        <div className="row row-note">
          {t("engineStatusLine", { engine: block.engineId, state: block.status.state })}
        </div>
      );
    case "crashed":
      return (
        <div className="row row-error">
          <strong>{t("engineCrashed", { engine: block.engineId })}</strong>
          {block.stderrTail.length > 0 ? (
            <details>
              <summary>{t("engineOutput")}</summary>
              <pre className="raw">{block.stderrTail.join("\n")}</pre>
            </details>
          ) : null}
        </div>
      );
    case "error":
      return (
        <div className="row row-error">
          <strong>
            {developer
              ? t("errorGeneric", { code: block.code, message: block.message })
              : t("errorGeneric", { next: block.nextAction ?? "" })}
          </strong>
          {developer && block.nextAction ? <span className="muted">{block.nextAction}</span> : null}
        </div>
      );
  }
});

export function Transcript() {
  const { transcript, turns, sessionId } = useStore();
  const t = useT();
  const blocks = blocksFor(transcript, turns);
  const end = useRef<HTMLDivElement>(null);

  // Follow the conversation as it grows.
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [blocks.length, transcript.length]);

  if (!sessionId) {
    return <p className="muted empty">{t("noSessionYet")}</p>;
  }

  return (
    <div className="transcript" role="log" aria-live="polite">
      {blocks.map((block) => (
        <Row key={block.key} block={block} />
      ))}
      <div ref={end} />
    </div>
  );
}
