// The request box (`07-ui-vocabulary.md` §3): Enter sends, Shift+Enter is a
// new line. While a turn runs it says so, with the latest thing the
// assistant did, and offers Stop — a spinner longer than two seconds with no
// words is not allowed (§4, rule 6).

import { useState, type KeyboardEvent } from "react";
import { ask, stop, useStore } from "../store";
import { latestActivity, blocksFor } from "../transcript";
import { useT } from "../vocab/useT";
import { describeCall } from "./ToolCallRow";
import { permissionTitle } from "./PermissionDialog";

export function Composer() {
  const { turnId, transcript, turns, projectId } = useStore();
  const t = useT();
  const [text, setText] = useState("");
  const running = turnId !== null;

  const latest = latestActivity(blocksFor(transcript, turns));
  const activity =
    latest?.kind === "tool"
      ? describeCall(t, latest.call) || null
      : latest?.kind === "permission"
        ? permissionTitle(t, latest.request)
        : null;

  const send = () => {
    const request = text.trim();
    if (!request || running || !projectId) return;
    setText("");
    void ask(request);
  };

  const onKey = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send();
    }
  };

  return (
    <div className="composer">
      {running ? (
        <div className="working" aria-live="polite">
          <span className="pulse" aria-hidden="true" />
          <span>{t("working")}</span>
          {activity ? <span className="muted">{activity}</span> : null}
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
        <button type="button" disabled title={t("planItSoon")}>
          {t("planIt")}
        </button>
        <button type="button" className="primary" disabled={running || !text.trim()} onClick={send}>
          {t("askDirect")}
        </button>
      </div>
    </div>
  );
}
