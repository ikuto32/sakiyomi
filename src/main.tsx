import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { getState, init, toast } from "./store";
import { applySettings, resolvedLang, resolvedNotation, watchSystemTheme } from "./settings";
import { t } from "./i18n";
import { useStore } from "./store";

/** Remount the UI when the language or notation changes, so every string re-renders. */
function Root() {
  const key = useStore((s) => resolvedLang(s.settings) + ":" + resolvedNotation(s.settings));
  return <App key={key} />;
}
import "./styles.css";

applySettings(getState().settings);
watchSystemTheme(() => getState().settings);

init()
  .catch((e) => toast(t("初期化に失敗しました: {e}", { e: String(e) }), true))
  .finally(() => {
    ReactDOM.createRoot(document.getElementById("root")!).render(
      <React.StrictMode>
        <Root />
      </React.StrictMode>,
    );
  });
