// The question only a person can answer (`07-ui-vocabulary.md` §3).
//
// Modal, one at a time, the rest queued behind it. The title says what kind
// of thing is being asked; the body is the facts the engine gave; the buttons
// are the options the engine actually offered, in this mode's words. Esc is
// "Don't": a question dismissed is a question answered no, never yes. A
// destructive request starts with focus on Reject.

import { useEffect, useRef } from "react";
import type { Decision, PermissionView } from "../types";
import { answer, useStore } from "../store";
import { useT, type T } from "../vocab/useT";

export function permissionTitle(t: T, request: PermissionView): string {
  const what = request.title;
  switch (request.risk) {
    case "outbound":
      return t("permOutbound", { what });
    case "destructive":
      return t("permDestructive", { what });
    case "execute":
      return t("permExecute", { what });
    default:
      return t("permOther", { what, risk: request.risk });
  }
}

/**
 * Which decisions to offer. "Always" is only offered where it can be safe:
 * never for something that leaves the computer or that Undo cannot reach.
 * The full decision table arrives with M4-T02.
 */
function offered(request: PermissionView): Decision[] {
  const kinds = new Set(request.options.map((option) => option.kind));
  const always = request.risk !== "outbound" && request.risk !== "destructive";
  const decisions: Decision[] = [];
  if (kinds.has("allow_once")) decisions.push("allow_once");
  if (always && kinds.has("allow_always")) decisions.push("allow_always");
  if (kinds.has("reject_once")) decisions.push("reject_once");
  else if (kinds.has("reject_always")) decisions.push("reject_always");
  return decisions;
}

export function PermissionDialog() {
  const { asking } = useStore();
  const t = useT();
  const request = asking[0];
  const dialog = useRef<HTMLDivElement>(null);
  const rejectButton = useRef<HTMLButtonElement>(null);
  const allowButton = useRef<HTMLButtonElement>(null);

  // Focus: Reject for anything Undo cannot take back, Allow otherwise. And
  // the trap: Tab cycles inside the dialog while it is up.
  useEffect(() => {
    if (!request) return;
    const dangerous = request.risk === "outbound" || request.risk === "destructive";
    (dangerous ? rejectButton : allowButton).current?.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void answer(request.request_id, "reject_once");
        return;
      }
      if (event.key !== "Tab" || !dialog.current) return;
      const focusable = dialog.current.querySelectorAll<HTMLElement>(
        "button:not([disabled]), [href], input, textarea, [tabindex]:not([tabindex='-1'])",
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [request]);

  if (!request) return null;

  const decisions = offered(request);
  const label = (decision: Decision) =>
    decision === "allow_once"
      ? t("allowOnce")
      : decision === "allow_always"
        ? t("allowAlways")
        : t("reject");

  return (
    <div className="backdrop">
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="permission-title"
        ref={dialog}
      >
        <p className="dialog-kicker">{t("permissionTitle")}</p>
        <h2 id="permission-title">{permissionTitle(t, request)}</h2>
        {request.explanation ? <p className="dialog-body">{request.explanation}</p> : null}
        {request.locations.length > 0 ? (
          <div className="dialog-body">
            <span className="muted">{t("permissionWhere")}</span>
            <ul className="paths">
              {request.locations.map((path) => (
                <li key={path}>{path}</li>
              ))}
            </ul>
          </div>
        ) : null}
        <div className="dialog-buttons">
          {decisions.length === 0 ? (
            <p className="muted">{t("permissionNoOptions")}</p>
          ) : null}
          {decisions.map((decision) => (
            <button
              key={decision}
              type="button"
              ref={
                decision === "allow_once"
                  ? allowButton
                  : decision.startsWith("reject")
                    ? rejectButton
                    : undefined
              }
              className={decision.startsWith("allow") ? "primary" : ""}
              onClick={() => void answer(request.request_id, decision)}
            >
              {label(decision)}
            </button>
          ))}
        </div>
        {asking.length > 1 ? (
          <p className="muted small">{t("permissionQueued", { count: asking.length - 1 })}</p>
        ) : null}
      </div>
    </div>
  );
}
