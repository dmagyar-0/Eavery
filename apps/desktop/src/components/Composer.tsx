// The request box (`07-ui-vocabulary.md` §3): Enter plans, Shift+Enter is a
// new line. "Plan it" is the primary button — the plan gate is on by default
// in both modes (`06-plan-gate-permissions.md` §1) — and "Ask" / "Run" goes
// straight to work. While a turn runs it says so, with the latest thing the
// assistant did, and offers Stop — a spinner longer than two seconds with no
// words is not allowed (§4, rule 6). While a plan waits, it says that instead:
// the answer is on the plan card, not here.

import { useState, type KeyboardEvent } from "react";
import { ask, plan, stop, useStore } from "../store";
import { latestActivity, blocksFor } from "../transcript";
import { useT } from "../vocab/useT";
import { describeCall } from "./ToolCallRow";
import { permissionTitle } from "./PermissionDialog";

export function Composer() {
  const { turnId, transcript, turns, projectId, reviewing } = useStore();
  const t = useT();
  const [text, setText] = useState("");
  const running = turnId !== null;
  const waiting = reviewing !== null;

  const latest = latestActivity(blocksFor(transcript, turns));
  const activity =
    latest?.kind === "tool"
      ? describeCall(t, latest.call) || null
      : latest?.kind === "permission"
        ? permissionTitle(t, latest.request)
        : null;

  const send = (how: typeof ask) => {
    const request = text.trim();
    if (!request || running || !projectId) return;
    setText("");
    void how(request);
  };

  const onKey = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(plan);
    }
  };

  return (
    <div className="composer">
      {running ? (
        <div className="working" aria-live="polite">
          {waiting ? null : <span className="pulse" aria-hidden="true" />}
          <span>{waiting ? t("planWaiting") : t("working")}</span>
          {activity && !waiting ? <span className="muted">{activity}</span> : null}
          <button type="button" className="ghost" onClick={() => void stop()}>
            {t("stop")}
          </button>
        </div>
      ) : null}
      <textarea
        rows={3}
        value={text}
        placeholder={t("composerPlaceholder")}
        disabled={running}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={onKey}
        aria-label={t("composerPlaceholder")}
      />
      <div className="composer-buttons">
        <span className="muted small">{t("composerHint")}</span>
        <button
          type="button"
          disabled={running || !text.trim()}
          title={t("askDirectHint")}
          onClick={() => send(ask)}
        >
          {t("askDirect")}
        </button>
        <button
          type="button"
          className="primary"
          disabled={running || !text.trim()}
          title={t("planItHint")}
          onClick={() => send(plan)}
        >
          {t("planIt")}
        </button>
      </div>
    </div>
  );
}
