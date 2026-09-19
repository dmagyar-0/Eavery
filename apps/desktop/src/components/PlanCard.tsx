// The plan, as the person is asked to approve it (`07-ui-vocabulary.md` §3,
// `06-plan-gate-permissions.md` §2.4). The summary and the steps, and — always
// — what would leave the computer and what could not be undone, "Nothing" when
// nothing: those two lines are what the person is really being asked about.
// Below them, who saw the documents to make the plan. While the turn waits,
// the three answers; afterwards, the card stays in the transcript as it was.

import { useState } from "react";
import type { Plan } from "../types";
import { approvePlan, rejectPlan, useStore } from "../store";
import { useT } from "../vocab/useT";

function List({ title, items, none }: { title: string; items: string[]; none?: string }) {
  if (items.length === 0 && none === undefined) return null;
  return (
    <div className="plan-section">
      <h4>{title}</h4>
      {items.length === 0 ? (
        <p className="nothing">{none}</p>
      ) : (
        <ul>
          {items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function PlanCard({
  plan,
  vendor,
  turnId,
}: {
  plan: Plan;
  vendor: string;
  turnId: string | null;
}) {
  const t = useT();
  const { reviewing, settings } = useStore();
  const [edits, setEdits] = useState("");
  const developer = settings.mode === "developer";
  const waiting = reviewing !== null && reviewing.turnId === turnId;
  const structured = plan.steps.length > 0 || plan.files_touched.length > 0;

  return (
    <div className="plan-card" role={waiting ? "region" : undefined} aria-live={waiting ? "polite" : undefined}>
      <h3>{t("plan")}</h3>
      {plan.summary ? <p className="prose plan-summary">{plan.summary}</p> : null}
      {plan.steps.length > 0 ? (
        <ol className="plan-steps">
          {plan.steps.map((step, index) => (
            <li key={index}>{step.text}</li>
          ))}
        </ol>
      ) : null}
      {!structured && plan.raw_markdown ? <pre className="raw prose">{plan.raw_markdown}</pre> : null}
      <List title={t("planFiles")} items={plan.files_touched} />
      <List title={t("planOutbound")} items={plan.outbound} none={t("nothing")} />
      <List title={t("planIrreversible")} items={plan.irreversible} none={t("nothing")} />
      <List title={t("planWillNotDo")} items={plan.will_not_do} />
      {plan.user_edits ? (
        <div className="plan-section">
          <h4>{t("planEdits")}</h4>
          <p>{plan.user_edits}</p>
        </div>
      ) : null}
      {vendor ? <p className="muted small">{t("planSentTo", { vendor })}</p> : null}
      {developer && structured && plan.raw_markdown ? (
        <details>
          <summary>{t("planRaw")}</summary>
          <pre className="raw">{plan.raw_markdown}</pre>
        </details>
      ) : null}
      {waiting ? (
        <div className="plan-answer">
          <textarea
            rows={2}
            value={edits}
            placeholder={t("planEditsPlaceholder")}
            aria-label={t("planEditsPlaceholder")}
            onChange={(event) => setEdits(event.target.value)}
          />
          <div className="dialog-buttons">
            <button type="button" className="primary" onClick={() => void approvePlan(edits)}>
              {edits.trim() ? t("approveEdits") : t("approve")}
            </button>
            <button type="button" onClick={() => void rejectPlan()}>
              {t("cancel")}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
