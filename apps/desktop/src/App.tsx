// The window: one of three screens, the banner for what went wrong, and the
// dialogs that sit over everything (`07-ui-vocabulary.md` §1, §4).
//
// Keyboard (§4, rule 5): Esc closes the dialog that is up; Cmd/Ctrl+Z is the
// text field's own undo whenever one has focus, and only outside any input
// does it ask about undoing the last run.

import { useEffect, useRef, useState } from "react";
import "./styles/app.css";
import { go, goBackTo, lastFinishedTurn, start, useStore } from "./store";
import { useT } from "./vocab/useT";
import { Home } from "./screens/Home";
import { ProjectScreen } from "./screens/Project";
import { SettingsScreen } from "./screens/Settings";
import { PermissionDialog } from "./components/PermissionDialog";
import { TroubleBanner } from "./components/Trouble";
import type { Turn } from "./types";

function inTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target.isContentEditable
  );
}

function UndoConfirm({ turn, onClose }: { turn: Turn; onClose: () => void }) {
  const t = useT();
  const confirm = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirm.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="backdrop">
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="undo-title">
        <h2 id="undo-title">{t("undoConfirmTitle")}</h2>
        <p className="dialog-body">
          {t("undoConfirmBody", {
            request: turn.request,
            checkpoint: turn.pre_checkpoint ?? "",
          })}
        </p>
        <div className="dialog-buttons">
          <button
            type="button"
            className="primary"
            ref={confirm}
            onClick={() => {
              onClose();
              if (turn.pre_checkpoint) void goBackTo(turn.pre_checkpoint);
            }}
          >
            {t("undo")}
          </button>
          <button type="button" onClick={onClose}>
            {t("cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}

function App() {
  const { screen, projectId, projects, loading, asking, turnId } = useStore();
  const t = useT();
  const [undoing, setUndoing] = useState<Turn | null>(null);
  const project = projects.find((candidate) => candidate.id === projectId);

  useEffect(() => {
    void start();
  }, []);

  // Cmd/Ctrl+Z outside a text field: undo the last run, after asking.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const undoKey = (event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "z";
      if (!undoKey || inTextField(event.target)) return;
      if (screen !== "project" || turnId !== null || asking.length > 0 || undoing) return;
      const turn = lastFinishedTurn();
      if (!turn?.pre_checkpoint) return;
      event.preventDefault();
      setUndoing(turn);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [screen, turnId, asking.length, undoing]);

  return (
    <div className="app">
      <nav className="topbar" aria-label={t("appName")}>
        <button type="button" className="brand" onClick={() => go("home")}>
          {t("appName")}
        </button>
        <div className="crumbs">
          <button type="button" aria-current={screen === "home"} onClick={() => go("home")}>
            {t("navHome")}
          </button>
          {project ? (
            <button type="button" aria-current={screen === "project"} onClick={() => go("project")}>
              {project.name}
            </button>
          ) : null}
        </div>
        <button
          type="button"
          className="ghost"
          aria-current={screen === "settings"}
          onClick={() => go("settings")}
        >
          {t("navSettings")}
        </button>
      </nav>
      <TroubleBanner />
      {loading ? (
        <p className="muted empty">{t("loading")}</p>
      ) : screen === "settings" ? (
        <SettingsScreen />
      ) : screen === "project" && project ? (
        <ProjectScreen />
      ) : (
        <Home />
      )}
      <PermissionDialog />
      {undoing ? <UndoConfirm turn={undoing} onClose={() => setUndoing(null)} /> : null}
    </div>
  );
}

export default App;
