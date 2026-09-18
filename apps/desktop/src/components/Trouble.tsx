// The last thing that went wrong, as something to do about it
// (`07-ui-vocabulary.md` §4, rule 2): the next action is the headline in
// Everyday mode; the message and the code are for Developer mode.

import { dismissTrouble, useStore } from "../store";
import { useT } from "../vocab/useT";

export function TroubleBanner() {
  const { trouble, settings } = useStore();
  const t = useT();
  if (!trouble) return null;

  const developer = settings.mode === "developer";
  const headline = trouble.nextAction ?? trouble.message;
  return (
    <div className="trouble" role="alert">
      <div className="trouble-body">
        <strong>{developer ? t("troubleHeadline") : headline}</strong>
        {developer ? (
          <span className="trouble-detail">
            {trouble.code ? `${trouble.code}: ` : ""}
            {trouble.message}
            {trouble.nextAction ? ` — ${trouble.nextAction}` : ""}
          </span>
        ) : trouble.nextAction ? (
          <span className="trouble-detail">{trouble.message}</span>
        ) : null}
      </div>
      <button type="button" className="ghost" onClick={dismissTrouble}>
        {t("dismiss")}
      </button>
    </div>
  );
}
