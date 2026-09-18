// The Diagnostics panel (`03-architecture.md` §9, M3-T08): the version, the
// data folder, the log, and its tail, with one button that copies all of it.
// Developer mode only, and its job is to save the hours a "nothing happened"
// otherwise costs.

import { useEffect, useState } from "react";
import { refreshDiagnostics, useStore } from "../store";
import { useT } from "../vocab/useT";

function copyText(diagnostics: NonNullable<ReturnType<typeof useStore>["diagnostics"]>) {
  const head = [
    `Eavery ${diagnostics.version}`,
    `data: ${diagnostics.data_dir}`,
    `log: ${diagnostics.log_path}`,
    "",
  ];
  return [...head, ...diagnostics.log_tail].join("\n") + "\n";
}

export function Diagnostics() {
  const { diagnostics } = useStore();
  const t = useT();
  const [copied, setCopied] = useState<"no" | "yes" | "failed">("no");

  useEffect(() => {
    void refreshDiagnostics();
  }, []);

  const copy = async () => {
    if (!diagnostics) return;
    try {
      await navigator.clipboard.writeText(copyText(diagnostics));
      setCopied("yes");
    } catch {
      setCopied("failed");
    }
  };

  if (!diagnostics) return <p className="muted">{t("loading")}</p>;

  return (
    <section className="diagnostics" aria-label={t("diagnosticsTitle")}>
      <dl className="facts">
        <div>
          <dt>{t("diagnosticsVersion", { version: diagnostics.version })}</dt>
        </div>
        <div>
          <dt>{t("diagnosticsDataDir", { path: diagnostics.data_dir })}</dt>
        </div>
        <div>
          <dt>{t("diagnosticsLogPath", { path: diagnostics.log_path })}</dt>
        </div>
      </dl>
      <div className="row-buttons">
        <button type="button" onClick={() => void refreshDiagnostics()}>
          {t("refresh")}
        </button>
        <button type="button" onClick={() => void copy()}>
          {t("diagnosticsCopy")}
        </button>
        {copied === "yes" ? <span className="muted">{t("diagnosticsCopied")}</span> : null}
        {copied === "failed" ? <span className="muted">{t("diagnosticsCopyFailed")}</span> : null}
      </div>
      <p className="muted small">{t("diagnosticsLogTail", { count: diagnostics.log_tail.length })}</p>
      {diagnostics.log_tail.length === 0 ? (
        <p className="muted">{t("diagnosticsEmpty")}</p>
      ) : (
        <pre className="raw log">{diagnostics.log_tail.join("\n")}</pre>
      )}
    </section>
  );
}
