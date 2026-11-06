import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "./api";
import { EngineManager } from "./components/Dialogs";
import { Command, Pane, paneCommands, paneKeyHandlers } from "./components/Pane";
import { CommandPalette } from "./components/Palette";
import { StatusBar } from "./components/Resources";
import { SettingsDialog } from "./components/SettingsDialog";
import { ToolsDialog } from "./components/Tools";
import { Splitter } from "./components/Splitter";
import {
  Layout, getState, newSession, run, setActivePane, setGrid, setLayout, setPalette, setPane, setSettingsOpen, setToolsOpen, toast,
  toggleView, updateSettings, useStore,
} from "./store";
import { FONT_STEPS, PieceLabelContext, ZOOM_STEPS, step } from "./settings";
import { sessionTitle, t } from "./i18n";

const zoomBy = (dir: 1 | -1) => updateSettings({ zoom: step(ZOOM_STEPS, getState().settings.zoom, dir) });
const fontBy = (dir: 1 | -1) => updateSettings({ fontScale: step(FONT_STEPS, getState().settings.fontScale, dir) });

export function App() {
  const layout = useStore((s) => s.layout);
  const panes = useStore((s) => s.panes);
  const activePane = useStore((s) => s.activePane);
  const sessions = useStore((s) => s.sessions);
  const toasts = useStore((s) => s.toasts);
  const grid = useStore((s) => s.grid);
  const palette = useStore((s) => s.palette);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const toolsOpen = useStore((s) => s.toolsOpen);
  const running = useStore((s) => [...s.jobs.values()].filter((j) => !j.done).length);
  const pieceLabels = useStore((s) => s.settings.pieceLabels);
  const [engineDialog, setEngineDialog] = useState(false);

  const globalCommands = (): Command[] => [
    ...([1, 2, 4] as Layout[]).map((n) => ({ id: `grid.${n}`, title: t("マルチビュー: {n}面表示", { n }), run: () => run(setLayout(n)) })),
    { id: "grid.reset", title: t("マルチビュー: ビューの大きさを均等にする"), run: () => setGrid({ two: [0.5, 0.5], rows: [0.5, 0.5], top: [0.5, 0.5], bottom: [0.5, 0.5] }) },
    ...[0, 1, 2, 3].filter((i) => i < getState().layout).map((i) => ({ id: `focus.${i}`, title: t("ビュー {n} を選択", { n: i + 1 }), keys: `Ctrl+${i + 1}`, run: () => setActivePane(i) })),
    { id: "session.new", title: t("新しい対局タブ"), keys: "Ctrl+T", run: () => addSession() },
    { id: "engines", title: t("エンジン管理…"), run: () => setEngineDialog(true) },
    { id: "tools", title: t("ツール: 定跡・棋譜・局面の変換、定跡掘り…"), run: () => setToolsOpen(true) },
    { id: "settings", title: t("設定を開く…"), keys: "Ctrl+,", run: () => setSettingsOpen(true) },
    { id: "theme.system", title: t("テーマ: システムに合わせる"), run: () => updateSettings({ theme: "system" }) },
    { id: "theme.dark", title: t("テーマ: ダーク"), run: () => updateSettings({ theme: "dark" }) },
    { id: "theme.light", title: t("テーマ: ライト"), run: () => updateSettings({ theme: "light" }) },
    { id: "pieces.one", title: t("駒の文字: 1文字（歩・龍）"), run: () => updateSettings({ pieceLabels: "one" }) },
    { id: "pieces.two", title: t("駒の文字: 2文字（歩兵・龍王）"), run: () => updateSettings({ pieceLabels: "two" }) },
    { id: "lang.ja", title: t("言語: 日本語"), run: () => updateSettings({ language: "ja" }) },
    { id: "lang.en", title: t("言語: English"), run: () => updateSettings({ language: "en" }) },
    { id: "notation.ja", title: t("棋譜表記: 日本語（▲７六歩）"), run: () => updateSettings({ notation: "ja" }) },
    { id: "notation.western", title: t("棋譜表記: 西洋式（☗P-7f）"), run: () => updateSettings({ notation: "western" }) },
    { id: "notation.usi", title: t("棋譜表記: USI（7g7f）"), run: () => updateSettings({ notation: "usi" }) },
    { id: "zoom.in", title: t("表示: 拡大"), keys: "Ctrl+=", run: () => zoomBy(1) },
    { id: "zoom.out", title: t("表示: 縮小"), keys: "Ctrl+-", run: () => zoomBy(-1) },
    { id: "zoom.reset", title: t("表示: 100%に戻す"), keys: "Ctrl+0", run: () => updateSettings({ zoom: 1 }) },
    { id: "font.up", title: t("文字サイズ: 大きく"), keys: "Ctrl+Shift+=", run: () => fontBy(1) },
    { id: "font.down", title: t("文字サイズ: 小さく"), keys: "Ctrl+Shift+-", run: () => fontBy(-1) },
  ];

  // Keys go to the pane the user last clicked, never to the one under the mouse.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      if ((ctrl && e.shiftKey && e.key.toLowerCase() === "p") || e.key === "F1") {
        e.preventDefault();
        setPalette(true);
        return;
      }
      // Zoom / font size work everywhere, like VS Code.
      if (ctrl && !e.altKey) {
        // Physical keys first, so JIS layouts (where "=" is Shift+"-") behave like US ones.
        const minus = e.code === "Minus" || e.code === "NumpadSubtract" || (!e.code && e.key === "-");
        const plus = !minus && (e.code === "Equal" || e.code === "Semicolon" || e.code === "NumpadAdd" || (!e.code && (e.key === "=" || e.key === "+")));
        if (plus || minus) {
          e.preventDefault();
          (e.shiftKey ? fontBy : zoomBy)(plus ? 1 : -1);
          return;
        }
        if (e.key === "0" || e.code === "Numpad0") {
          e.preventDefault();
          updateSettings(e.shiftKey ? { fontScale: 1 } : { zoom: 1 });
          return;
        }
        if (e.key === ",") {
          e.preventDefault();
          setSettingsOpen(true);
          return;
        }
      }
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable]") || document.querySelector(".modal, .palette-backdrop")) return;
      const pi = getState().activePane;
      const cmd = (id: string) => paneCommands.get(pi)?.find((c) => c.id === id)?.run();
      if (ctrl && !e.shiftKey && !e.altKey) {
        const k = e.key.toLowerCase();
        if (k === "j") { e.preventDefault(); toggleView(pi, "thinking"); return; }
        if (k === "b") { e.preventDefault(); toggleView(pi, "moves"); return; }
        if (k === "o") { e.preventDefault(); cmd("kifu.open"); return; }
        if (k === "s") { e.preventDefault(); cmd("kifu.save"); return; }
        if (k === "v") { e.preventDefault(); cmd("kifu.paste"); return; }
        if (k === "t") { e.preventDefault(); addSession(); return; }
        if (/^[1-4]$/.test(k) && Number(k) <= getState().layout) { e.preventDefault(); setActivePane(Number(k) - 1); return; }
      }
      paneKeyHandlers.get(pi)?.(e);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Dropping a kifu file loads it into the pane it was dropped on.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent(async (e) => {
        if (e.payload.type !== "drop" || !e.payload.paths.length) return;
        const scale = window.devicePixelRatio || 1;
        const el = document.elementFromPoint(e.payload.position.x / scale, e.payload.position.y / scale);
        const paneEl = el?.closest("[data-pane]") as HTMLElement | null;
        const index = paneEl ? Number(paneEl.dataset.pane) : getState().activePane;
        const sid = getState().panes[index]?.sessionId;
        if (sid == null) return;
        setActivePane(index);
        try {
          await api.loadFile(sid, e.payload.paths[0]);
          toast(t("棋譜を読み込みました"));
        } catch (err) {
          toast(String(err), true);
        }
      })
      .then((f) => (unlisten = f));
    return () => unlisten?.();
  }, []);

  const showInActive = (id: number) => setPane(getState().activePane, { sessionId: id, editing: false });
  async function addSession() {
    const v = await newSession();
    if (v) showInActive(v.id);
  }

  const pane = (i: number) => <Pane key={i} index={i} />;
  const gridView =
    layout === 1 ? pane(0)
      : layout === 2 ? (
        <Splitter dir="row" sizes={grid.two} onSizes={(two) => setGrid({ two })}>{[pane(0), pane(1)]}</Splitter>
      ) : (
        <Splitter dir="col" sizes={grid.rows} onSizes={(rows) => setGrid({ rows })}>
          {[
            <Splitter key="t" dir="row" sizes={grid.top} onSizes={(top) => setGrid({ top })}>{[pane(0), pane(1)]}</Splitter>,
            <Splitter key="b" dir="row" sizes={grid.bottom} onSizes={(bottom) => setGrid({ bottom })}>{[pane(2), pane(3)]}</Splitter>,
          ]}
        </Splitter>
      );

  return (
    <PieceLabelContext.Provider value={pieceLabels}>
    <div className="app">
      <header className="topbar">
        <div className="brand" title="先読み — Sakiyomi"><span className="brand-mark">☗</span> Sakiyomi<span className="brand-sub">先読み</span></div>
        <nav className="tabs">
          {[...sessions.values()].map((s) => {
            const shownIn = panes.findIndex((p) => p.sessionId === s.id);
            const live = s.game && !s.game.over;
            return (
              <button
                key={s.id}
                className={"tab" + (panes[activePane]?.sessionId === s.id ? " on" : "") + (shownIn >= 0 ? " shown" : "")}
                onClick={() => showInActive(s.id)}
                title={t("クリックで選択中のビューに表示")}
              >
                {live && <span className="dot live" />}
                {s.analysis && <span className="dot analysis" />}
                {sessionTitle(s)}
                {shownIn >= 0 && layout > 1 && <span className="tab-pane">{shownIn + 1}</span>}
              </button>
            );
          })}
          <button className="tab add" onClick={addSession} title={t("新しい対局タブ (Ctrl+T)")}>＋</button>
        </nav>
        <button className="palette-button" onClick={() => setPalette(true)} title={t("コマンドパレット (Ctrl+Shift+P)")}>{t("⌘ コマンド")}</button>
        <div className="layout-switch" role="group" aria-label={t("マルチビュー")}>
          {([1, 2, 4] as Layout[]).map((n) => (
            <button key={n} className={layout === n ? "on" : ""} onClick={() => run(setLayout(n))} title={t("{n}面表示", { n })}>
              <LayoutIcon n={n} />
            </button>
          ))}
        </div>
        <button onClick={() => setToolsOpen(true)} title={t("定跡・棋譜・局面の変換、定跡掘り")}>{t("ツール")}{running > 0 && <span className="badge">{running}</span>}</button>
        <button onClick={() => setEngineDialog(true)}>{t("エンジン管理")}</button>
        <button className="icon gear" onClick={() => setSettingsOpen(true)} title={t("設定 (Ctrl+,)")} aria-label={t("設定")}>⚙</button>
      </header>
      <main className="panes">{gridView}</main>
      <StatusBar onResources={() => toggleView(getState().activePane, "resources")} />
      {engineDialog && <EngineManager onClose={() => setEngineDialog(false)} />}
      {settingsOpen && <SettingsDialog />}
      {toolsOpen && <ToolsDialog />}
      {palette && (
        <CommandPalette
          commands={[...(paneCommands.get(activePane) ?? []), ...globalCommands()]}
          onClose={() => setPalette(false)}
        />
      )}
      <div className="toasts">
        {toasts.map((t) => <div key={t.id} className={"toast" + (t.error ? " error" : "")}>{t.text}</div>)}
      </div>
    </div>
    </PieceLabelContext.Provider>
  );
}

function LayoutIcon({ n }: { n: number }) {
  return (
    <svg viewBox="0 0 20 14" width="20" height="14">
      {n === 1 && <rect x="1" y="1" width="18" height="12" rx="2" />}
      {n === 2 && <><rect x="1" y="1" width="8.5" height="12" rx="2" /><rect x="10.5" y="1" width="8.5" height="12" rx="2" /></>}
      {n === 4 && <><rect x="1" y="1" width="8.5" height="5.5" rx="1.5" /><rect x="10.5" y="1" width="8.5" height="5.5" rx="1.5" /><rect x="1" y="7.5" width="8.5" height="5.5" rx="1.5" /><rect x="10.5" y="7.5" width="8.5" height="5.5" rx="1.5" /></>}
    </svg>
  );
}
