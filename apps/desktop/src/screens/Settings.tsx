// Settings (`07-ui-vocabulary.md` §3): the mode, and the assistants. Only
// those two for M3; Connectors and Playbooks come with M6, install and
// sign-in buttons with M7. Developer mode adds the program paths and the
// Diagnostics panel.

import { useEffect } from "react";
import type { EngineListing, EngineStatus, UiMode } from "../types";
import { checkEngine, refreshEngines, saveSettings, useStore } from "../store";
import { useShown, useT, type T } from "../vocab/useT";
import { Diagnostics } from "../components/Diagnostics";

function stateChip(t: T, status: EngineStatus): string {
  return t(`engineState_${status.state}`);
}

/** The Everyday copy for each state (`07-ui-vocabulary.md` §5). */
function describe(t: T, engine: EngineListing): string {
  const name = engine.display_name;
  const status = engine.status;
  switch (status.state) {
    case "not_installed":
      return t("engineCopy_not_installed", { engine: name, instructions: status.instructions });
    case "needs_node":
      return t("engineCopy_needs_node", { engine: name });
    case "needs_sign_in":
      return t("engineCopy_needs_sign_in", { engine: name });
    case "installing":
      return t("engineCopy_installing", { engine: name, percent: status.percent });
    case "signing_in":
      return t("engineCopy_signing_in", { engine: name });
    case "ready":
      return t("engineCopy_ready");
    case "unavailable":
      return t("engineCopy_unavailable", { engine: name, reason: status.reason });
  }
}

function EngineRow({ engine }: { engine: EngineListing }) {
  const { checkingEngines } = useStore();
  const t = useT();
  const shown = useShown();
  const checking = checkingEngines.includes(engine.id);
  const status = engine.status;

  return (
    <li className={`engine engine-${status.state}`}>
      <div className="engine-head">
        <span className="engine-name">
          {engine.display_name}
          {engine.experimental ? <span className="chip">{t("experimentalTag")}</span> : null}
        </span>
        <span className={`chip state-${status.state}`}>{stateChip(t, status)}</span>
        <button
          type="button"
          className="ghost"
          disabled={checking}
          onClick={() => void checkEngine(engine.id)}
        >
          {checking ? t("checking") : t("checkAgain")}
        </button>
      </div>
      <p className="engine-copy">{describe(t, engine)}</p>
      {status.state === "needs_sign_in" ? <pre className="raw command">{status.command}</pre> : null}
      {status.state === "ready" && shown("engineVersion") ? (
        <p className="muted small">
          {t("engineVersion", {
            name: status.info.name ?? engine.id,
            version: status.info.version ?? "",
            protocol: status.info.protocol_version,
          })}
          {status.modes.length > 0
            ? " · " + t("engineModes", { modes: status.modes.map((mode) => mode.id).join(", ") })
            : ""}
        </p>
      ) : null}
      {shown("programPath") && engine.program ? (
        <p className="muted small path">{t("programPath", { path: engine.program })}</p>
      ) : null}
    </li>
  );
}

export function SettingsScreen() {
  const { settings, engines } = useStore();
  const t = useT();
  const shown = useShown();

  // Whenever the mode changes, the list may gain or lose experimental rows.
  useEffect(() => {
    void refreshEngines();
  }, [settings.mode]);

  const setMode = (mode: UiMode) => void saveSettings({ ...settings, mode });
  const setDefault = (engineId: string) =>
    void saveSettings({ ...settings, default_engine: engineId || null });

  return (
    <main className="screen settings">
      <header className="screen-head">
        <h1>{t("settingsTitle")}</h1>
      </header>

      <section aria-labelledby="mode-title">
        <h2 id="mode-title">{t("settingsMode")}</h2>
        <div className="segmented" role="radiogroup" aria-labelledby="mode-title">
          <button
            type="button"
            role="radio"
            aria-checked={settings.mode === "everyday"}
            onClick={() => setMode("everyday")}
          >
            {t("modeEveryday")}
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={settings.mode === "developer"}
            onClick={() => setMode("developer")}
          >
            {t("modeDeveloper")}
          </button>
        </div>
        <p className="muted">{t("modeHint")}</p>
      </section>

      <section aria-labelledby="engines-title">
        <h2 id="engines-title">{t("settingsEngines")}</h2>
        <p className="muted">{t("settingsEnginesLead")}</p>
        <label className="field">
          <span>{t("defaultEngine")}</span>
          <select
            value={settings.default_engine ?? ""}
            onChange={(event) => setDefault(event.target.value)}
          >
            <option value="">{t("noDefaultEngine")}</option>
            {engines.map((engine) => (
              <option key={engine.id} value={engine.id}>
                {engine.display_name}
              </option>
            ))}
          </select>
        </label>
        {engines.length === 0 ? <p className="muted">{t("checking")}</p> : null}
        <ul className="engine-list">
          {engines.map((engine) => (
            <EngineRow key={engine.id} engine={engine} />
          ))}
        </ul>
      </section>

      {shown("diagnosticsTitle") ? (
        <section aria-labelledby="diagnostics-title">
          <h2 id="diagnostics-title">{t("diagnosticsTitle")}</h2>
          <Diagnostics />
        </section>
      ) : null}
    </main>
  );
}
