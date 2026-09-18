// The history, and the way back (`05-git-journal.md` §6, `07-ui-vocabulary.md` §3).
//
// Newest first. Selecting one asks the core what going back there would
// change — against the folder as it is now, so the person's own edits count
// — and offers the button. Undo is the last finished turn's pre-turn
// checkpoint; Redo, after a restore, is where the files were just before it.

import { useEffect, useState } from "react";
import type { ChangeSet, Checkpoint } from "../types";
import {
  changesSince,
  goBackTo,
  lastFinishedTurn,
  protectNow,
  useStore,
} from "../store";
import { explain } from "../ipc";
import { useT, type T } from "../vocab/useT";

function when(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? iso
    : date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

function kindLabel(t: T, checkpoint: Checkpoint): string {
  switch (checkpoint.kind) {
    case "pre_turn":
      return t("checkpointKind_pre_turn");
    case "post_turn":
      return t("checkpointKind_post_turn");
    case "manual":
      return t("checkpointKind_manual");
    case "restore":
      return t("checkpointKind_restore");
  }
}

function Preview({ checkpointId, busy }: { checkpointId: string; busy: boolean }) {
  const t = useT();
  const [changes, setChanges] = useState<ChangeSet | null | "checking">("checking");
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setChanges("checking");
    setFailed(null);
    const asked = changesSince(checkpointId);
    if (!asked) return;
    asked
      .then((result) => {
        if (!cancelled) setChanges(result);
      })
      .catch((error: unknown) => {
        if (!cancelled) setFailed(explain(error).message);
      });
    return () => {
      cancelled = true;
    };
  }, [checkpointId]);

  const files =
    changes === "checking" || changes === null
      ? []
      : [...changes.added, ...changes.changed, ...changes.removed];

  return (
    <div className="preview">
      {failed ? (
        <p className="muted">{failed}</p>
      ) : changes === "checking" ? (
        <p className="muted">{t("goingBackChecking")}</p>
      ) : files.length === 0 ? (
        <p className="muted">{t("goingBackNothing")}</p>
      ) : (
        <>
          <p>{t("goingBackWouldChange")}</p>
          <ul className="paths">
            {files.map((file) => (
              <li key={file}>{file}</li>
            ))}
          </ul>
        </>
      )}
      <button
        type="button"
        className="primary"
        disabled={busy || changes === "checking"}
        onClick={() => void goBackTo(checkpointId)}
      >
        {t("goBackHere")}
      </button>
    </div>
  );
}

export function Checkpoints() {
  const { checkpoints, turnId, redo, turns } = useStore();
  const t = useT();
  const [selected, setSelected] = useState<string | null>(null);
  const busy = turnId !== null;

  // `turns` is read so the Undo button follows the transcript; the store
  // decides which turn that is.
  const undoable = turns.length > 0 ? lastFinishedTurn() : null;

  return (
    <section className="checkpoints" aria-label={t("checkpoints")}>
      <div className="row-buttons">
        {undoable?.pre_checkpoint ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => void goBackTo(undoable.pre_checkpoint as string)}
            title={undoable.request}
          >
            {t("undoLastRun")}
          </button>
        ) : null}
        {redo ? (
          <button type="button" disabled={busy} onClick={() => void goBackTo(redo.to)}>
            {t("redoLastUndo")}
          </button>
        ) : null}
        <button
          type="button"
          className="ghost"
          disabled={busy}
          onClick={() => void protectNow(t("protectNowLabel"))}
        >
          {t("protectNow")}
        </button>
      </div>
      <p className="muted small">{t("ownEditsNote")}</p>
      {checkpoints.length === 0 ? <p className="muted">{t("noCheckpoints")}</p> : null}
      <ol className="checkpoint-list">
        {checkpoints.map((checkpoint) => {
          const isSelected = selected === checkpoint.id;
          return (
            <li key={checkpoint.id} className={isSelected ? "selected" : ""}>
              <button
                type="button"
                className="checkpoint"
                aria-expanded={isSelected}
                onClick={() => setSelected(isSelected ? null : checkpoint.id)}
              >
                <span className="checkpoint-label">{checkpoint.label}</span>
                <span className="checkpoint-meta">
                  <span className="chip">{kindLabel(t, checkpoint)}</span>
                  <span>
                    {checkpoint.files_changed === 1
                      ? t("fileChangedOne")
                      : t("filesChanged", { count: checkpoint.files_changed })}
                  </span>
                  <span>{when(checkpoint.created_at)}</span>
                </span>
              </button>
              {isSelected ? <Preview checkpointId={checkpoint.id} busy={busy} /> : null}
            </li>
          );
        })}
      </ol>
    </section>
  );
}
