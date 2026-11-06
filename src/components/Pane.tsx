import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, SessionView, TextEncoding, ThinkingView } from "../api";
import {
  applyDockToAll, closeSession, getState, run, runOk, setActivePane, setDock, setPane, toast, toggleView, useStore,
} from "../store";
import { ALL_VIEWS, LayoutNode, PRESETS, ViewId, viewTitle, views as viewsOf } from "../layout";
import { indexLegal } from "../shogi";
import { Board } from "./Board";
import { BoardEditor } from "./BoardEditor";
import { AnalysisDialog, NewGameDialog, TextDialog } from "./Dialogs";
import { Dock } from "./Dock";
import { BookView } from "./BookView";
import { EvalGraph } from "./EvalGraph";
import { CommentView, MoveLineView, MoveTreeView } from "./MoveList";
import { ResourcesView } from "./Resources";
import { Clocks, ThinkingPanel } from "./Thinking";
import { playerName, sessionTitle, t } from "../i18n";

/** Keyboard handlers by pane index; App routes keys to the *active* (clicked) pane only. */
export const paneKeyHandlers = new Map<number, (e: KeyboardEvent) => void>();

export interface Command {
  id: string;
  title: string;
  keys?: string;
  run: () => void;
}
/** Commands each pane offers to the command palette. */
export const paneCommands = new Map<number, Command[]>();

type DialogKind = "game" | "analysis" | "paste" | { text: string; title: string } | null;

const SAVE_FORMATS: Record<string, { label: string; ext: string; format: string; encoding: TextEncoding }> = {
  spg: { label: "SPG形式（標準）", ext: "spg", format: "spg", encoding: "utf8" },
  spgt: { label: "SPGテキスト形式（.spgt）", ext: "spgt", format: "spgt", encoding: "utf8" },
  kifSjis: { label: "KIF形式（Shift_JIS）", ext: "kif", format: "kif", encoding: "sjis" },
  kifUtf8: { label: "KIF形式（UTF-8 .kifu）", ext: "kifu", format: "kif", encoding: "utf8" },
  ki2: { label: "KI2形式（Shift_JIS）", ext: "ki2", format: "ki2", encoding: "sjis" },
  csa: { label: "CSA形式", ext: "csa", format: "csa", encoding: "utf8" },
  jkf: { label: "JKF形式（JSON）", ext: "jkf", format: "jkf", encoding: "utf8" },
};

function Menu({ label, children }: { label: string; children: React.ReactNode }) {
  const [openState, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!openState) return;
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [openState]);
  return (
    <div className="menu" ref={ref}>
      <button onClick={() => setOpen(!openState)} className={openState ? "on" : ""}>{label} ▾</button>
      {openState && <div className="menu-items" onClick={() => setOpen(false)}>{children}</div>}
    </div>
  );
}

export function Pane({ index }: { index: number }) {
  const pane = useStore((s) => s.panes[index]);
  const isActive = useStore((s) => s.activePane === index);
  const multi = useStore((s) => s.layout > 1);
  const sessions = useStore((s) => s.sessions);
  const v = pane?.sessionId != null ? sessions.get(pane.sessionId) : undefined;
  const thinkingMap = useStore((s) => (pane?.sessionId != null ? s.thinking.get(pane.sessionId) : undefined));
  const [dialog, setDialog] = useState<DialogKind>(null);

  if (!pane || !v) {
    return <div className="pane empty" data-pane={index} onMouseDown={() => setActivePane(index)}>{t("セッションがありません")}</div>;
  }
  return (
    <PaneBody
      index={index} v={v} flipped={pane.flipped} editing={pane.editing} dock={pane.dock}
      isActive={isActive} multi={multi} sessions={sessions} thinkingMap={thinkingMap}
      dialog={dialog} setDialog={setDialog}
    />
  );
}

function PaneBody({ index, v, flipped, editing, dock, isActive, multi, sessions, thinkingMap, dialog, setDialog }: {
  index: number; v: SessionView; flipped: boolean; editing: boolean; dock: LayoutNode; isActive: boolean; multi: boolean;
  sessions: Map<number, SessionView>; thinkingMap: Map<number, ThinkingView> | undefined;
  dialog: DialogKind; setDialog: (d: DialogKind) => void;
}) {
  const id = v.id;
  const game = v.game;
  const playing = !!game && !game.over;
  const legal = useMemo(() => (v.legal.length ? indexLegal(v.legal) : null), [v.legal]);
  const node = v.nodes[v.cursor];
  const [renaming, setRenaming] = useState(false);
  const shown = viewsOf(dock);

  // Only engines that still belong to this session.
  const thinking = useMemo(() => {
    const keys = new Set(v.engines.map((e) => e.key));
    return [...(thinkingMap?.values() ?? [])].filter((t) => keys.has(t.key)).sort((a, b) => a.key - b.key);
  }, [thinkingMap, v.engines]);
  const analysisView = thinking.find((t) => t.role === "analysis");
  const live = analysisView?.lines[0]?.eval ? { node: analysisView.node, eval: analysisView.lines[0].eval } : null;
  const arrow = analysisView && analysisView.node === v.cursor ? analysisView.lines[0]?.pvUsi[0] ?? null : null;

  const select = useCallback((n: number) => run(api.setCursor(id, n)), [id]);
  const lineEnd = () => {
    let n = v.cursor;
    while (v.nodes[n].children.length) n = v.nodes[n].children[0];
    return n;
  };

  const navigate = useCallback((e: KeyboardEvent) => {
    const cur = v.nodes[v.cursor];
    const sib = cur.parent !== null ? v.nodes[cur.parent].children : [v.cursor];
    const i = sib.indexOf(v.cursor);
    let target: number | null = null;
    switch (e.key) {
      case "ArrowLeft": target = cur.parent; break;
      case "ArrowRight": target = cur.children[0] ?? null; break;
      case "ArrowUp": target = i > 0 ? sib[i - 1] : null; break;
      case "ArrowDown": target = i < sib.length - 1 ? sib[i + 1] : null; break;
      case "Home": target = 0; break;
      case "End": {
        let n = v.cursor;
        while (v.nodes[n].children.length) n = v.nodes[n].children[0];
        target = n;
        break;
      }
      default: return;
    }
    e.preventDefault();
    if (target !== null && target !== v.cursor) select(target);
  }, [v.nodes, v.cursor, select]);

  useEffect(() => {
    if (editing) {
      paneKeyHandlers.delete(index);
      return;
    }
    paneKeyHandlers.set(index, navigate);
    return () => {
      if (paneKeyHandlers.get(index) === navigate) paneKeyHandlers.delete(index);
    };
  }, [index, navigate, editing]);

  const openFile = async () => {
    const path = await open({ multiple: false, filters: [{ name: t("棋譜"), extensions: ["spg", "spgt", "kif", "kifu", "ki2", "ki2u", "csa", "jkf", "sfen", "usi"] }, { name: t("すべて"), extensions: ["*"] }] });
    if (typeof path !== "string") return;
    const r = await run(api.loadFile(id, path));
    if (!r) return;
    toast(r.opened > 1 ? t("{n}局の棋譜を読み込みました", { n: r.opened }) : t("棋譜を読み込みました"));
    if (r.notice) toast(r.notice);
  };
  const saveAs = async (key: string) => {
    const f = SAVE_FORMATS[key];
    const path = await save({ defaultPath: `${sessionTitle(v)}.${f.ext}`, filters: [{ name: t(f.label), extensions: [f.ext] }] });
    if (path && (await runOk(api.saveFile(id, path, f.format, f.encoding, getState().settings.evalStyle)))) toast(t("保存しました: {path}", { path }));
  };
  const copy = async (what: "kif" | "ki2" | "csa" | "jkf" | "sfen" | "usi" | "bod" | "spgt") => {
    const text = await run(api.exportText(id, what, what === "sfen" || what === "usi" || what === "bod" ? "none" : getState().settings.evalStyle));
    if (text === undefined) return;
    await writeText(text);
    toast(t("{what} をコピーしました", { what: what.toUpperCase() }));
  };
  const paste = async () => {
    let text = "";
    try {
      text = await readText();
    } catch { /* empty clipboard */ }
    if (!text.trim()) return setDialog("paste");
    if (await runOk(api.importText(id, text))) toast(t("貼り付けました"));
  };
  const showText = async (what: "kif" | "ki2" | "csa" | "spgt") => {
    const text = await run(api.exportText(id, what, getState().settings.evalStyle));
    if (text !== undefined) setDialog({ text, title: t("{what} 形式", { what: what.toUpperCase() }) });
  };

  // Commands for the palette (the palette only asks the active pane).
  const commands: Command[] = [
    ...ALL_VIEWS.map((view) => ({
      id: `view.${view}`,
      title: t(shown.includes(view) ? "表示: {name}を隠す" : "表示: {name}を表示", { name: viewTitle(view) }),
      keys: view === "thinking" ? "Ctrl+J" : view === "moves" ? "Ctrl+B" : undefined,
      run: () => toggleView(index, view),
    })),
    ...Object.entries(PRESETS).map(([k, p]) => ({ id: `layout.${k}`, title: t("レイアウト: {name}", { name: t(p.title) }), run: () => setDock(index, p.make()) })),
    { id: "layout.all", title: t("レイアウト: この配置を全ビューに適用"), run: () => applyDockToAll(index) },
    ...(!playing ? [{ id: "game.new", title: t("対局: 新しい対局…"), run: () => setDialog("game") }] : [
      { id: "game.stop", title: t("対局: 中断"), run: () => run(api.stopGame(id)) },
      { id: "game.resign", title: t("対局: 投了"), run: () => run(api.resign(id)) },
      { id: "game.movenow", title: t("対局: 今すぐ指させる"), run: () => run(api.moveNow(id)) },
    ]),
    v.analysis
      ? { id: "analysis.stop", title: t("検討: 停止"), run: () => run(api.stopAnalysis(id)) }
      : { id: "analysis.start", title: t("検討: 開始…"), run: () => setDialog("analysis") },
    { id: "board.flip", title: t("盤面: 反転"), run: () => setPane(index, { flipped: !flipped }) },
    { id: "board.edit", title: t("盤面: 局面編集"), run: () => !playing && setPane(index, { editing: !editing }) },
    { id: "kifu.open", title: t("棋譜: 開く…"), keys: "Ctrl+O", run: openFile },
    { id: "kifu.save", title: t("棋譜: SPG形式で保存…"), keys: "Ctrl+S", run: () => saveAs("spg") },
    { id: "kifu.saveKif", title: t("棋譜: KIF形式で保存…"), run: () => saveAs("kifSjis") },
    { id: "kifu.copy.kif", title: t("棋譜: KIFをコピー"), run: () => copy("kif") },
    { id: "kifu.copy.sfen", title: t("棋譜: SFENをコピー"), run: () => copy("sfen") },
    { id: "kifu.copy.usi", title: t("棋譜: USI positionをコピー"), run: () => copy("usi") },
    { id: "kifu.paste", title: t("棋譜: 貼り付け"), keys: "Ctrl+V", run: paste },
  ];
  paneCommands.set(index, commands);
  useEffect(() => () => void paneCommands.delete(index), [index]);

  const others = [...sessions.values()];
  const lastMove = node.usi ? { from: node.from, to: node.to } : null;
  const humanSide = playing && game!.humanToMove;

  const renderView = (view: ViewId): React.ReactNode => {
    switch (view) {
      case "board":
        return editing ? (
          <BoardEditor v={v} flipped={flipped} onDone={() => setPane(index, { editing: false })} />
        ) : (
          <div className="board-wrap">
            <Board
              squares={v.board.squares}
              hands={v.board.hands}
              side={v.board.side}
              flipped={flipped}
              names={[game?.players[0].name || playerName(v, 0), game?.players[1].name || playerName(v, 1)]}
              legal={legal}
              onMove={(usi) => run(api.makeMove(id, usi))}
              lastMove={lastMove}
              inCheck={v.board.inCheck}
              arrow={arrow}
              activeSide={playing ? game!.running : v.board.side}
            />
          </div>
        );
      case "graph":
        return <EvalGraph v={v} live={live} onSelect={select} />;
      case "moves":
        return <MoveLineView v={v} onSelect={select} />;
      case "tree":
        return <MoveTreeView v={v} onSelect={select} />;
      case "comment":
        return <CommentView v={v} />;
      case "thinking":
        return <ThinkingPanel v={v} views={thinking} />;
      case "clocks":
        return game ? <Clocks v={v} game={game} flipped={flipped} /> : <div className="placeholder">{t("対局中ではありません")}</div>;
      case "book":
        return <BookView v={v} onPlay={(usi) => !playing || humanSide ? run(api.makeMove(id, usi)) : toast(t("エンジンの手番です"), true)} />;
      case "resources":
        return <ResourcesView v={v} />;
    }
  };

  return (
    <div
      className={"pane" + (isActive ? " active" : "") + (multi ? " multi" : "")}
      data-pane={index}
      onMouseDown={() => setActivePane(index)}
    >
      <div className="pane-header">
        <select className="session-select" value={id} onChange={(e) => setPane(index, { sessionId: Number(e.target.value), editing: false })} title={t("このビューに表示する対局")}>
          {others.map((s) => <option key={s.id} value={s.id}>{sessionTitle(s)}{s.game && !s.game.over ? " ●" : s.analysis ? " ◎" : ""}</option>)}
        </select>
        {renaming ? (
          <input
            className="rename"
            autoFocus
            defaultValue={sessionTitle(v)}
            onBlur={(e) => { setRenaming(false); if (e.target.value && e.target.value !== sessionTitle(v)) run(api.setTitle(id, e.target.value)); }}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        ) : (
          <span className="pane-title" onDoubleClick={() => setRenaming(true)} title={t("ダブルクリックで名前を変更")}>
            {t("{a} vs {b}", { a: playerName(v, 0), b: playerName(v, 1) })}
          </span>
        )}
        <span className="status">{v.busy && <span className="spinner" />}{v.status}</span>
        <button className="icon" title={t("この対局を閉じる（エンジンも終了）")} onClick={() => closeSession(id)}>✕</button>
      </div>

      <div className="toolbar">
        {playing ? (
          <>
            <button className="danger" onClick={() => run(api.stopGame(id))}>{t("中断")}</button>
            <button disabled={!humanSide} onClick={() => run(api.resign(id))}>{t("投了")}</button>
            <button disabled={humanSide} onClick={() => run(api.moveNow(id))} title={t("エンジンに今すぐ指させる")}>{t("今すぐ指させる")}</button>
          </>
        ) : (
          <button className="primary" disabled={v.busy || editing} onClick={() => setDialog("game")}>{t("対局")}</button>
        )}
        {v.analysis ? (
          <button className="accent" onClick={() => run(api.stopAnalysis(id))}>{t("検討停止")}</button>
        ) : (
          <button disabled={playing || v.busy || editing} onClick={() => setDialog("analysis")}>{t("検討")}</button>
        )}
        <span className="sep" />
        <button className="icon" title={t("開始局面 (Home)")} onClick={() => select(0)}>⏮</button>
        <button className="icon" title={t("1手戻る (←)")} disabled={node.parent === null} onClick={() => node.parent !== null && select(node.parent)}>◀</button>
        <button className="icon" title={t("1手進む (→)")} disabled={!node.children.length} onClick={() => node.children.length && select(node.children[0])}>▶</button>
        <button className="icon" title={t("最終局面 (End)")} onClick={() => select(lineEnd())}>⏭</button>
        <span className="sep" />
        <button title={t("盤面反転")} onClick={() => setPane(index, { flipped: !flipped })}>{t("⇅ 反転")}</button>
        <button className={editing ? "on" : ""} disabled={playing || v.busy} onClick={() => setPane(index, { editing: !editing })}>{t("局面編集")}</button>
        <Menu label={t("棋譜")}>
          <button onClick={openFile}>{t("開く…")}</button>
          {Object.entries(SAVE_FORMATS).map(([k, f]) => (
            <button key={k} onClick={() => saveAs(k)}>{t("{fmt}で保存…", { fmt: t(f.label) })}</button>
          ))}
          <hr />
          <button onClick={() => copy("kif")}>{t("KIFをコピー")}</button>
          <button onClick={() => copy("ki2")}>{t("KI2をコピー")}</button>
          <button onClick={() => copy("csa")}>{t("CSAをコピー")}</button>
          <button onClick={() => copy("spgt")}>{t("SPGテキストをコピー")}</button>
          <button onClick={() => copy("sfen")}>{t("局面のSFENをコピー")}</button>
          <button onClick={() => copy("usi")}>{t("USI position をコピー")}</button>
          <button onClick={() => copy("bod")}>{t("局面図（BOD）をコピー")}</button>
          <button onClick={paste}>{t("貼り付け（棋譜／SFEN／position）")}</button>
          <hr />
          <button onClick={() => showText("kif")}>{t("KIFをテキスト表示")}</button>
          <button onClick={() => showText("spgt")}>{t("SPGテキストを表示")}</button>
          <button onClick={() => setDialog("paste")}>{t("テキストから読み込む…")}</button>
        </Menu>
        <Menu label={t("表示")}>
          {ALL_VIEWS.map((view) => (
            <button key={view} className="check" onClick={() => toggleView(index, view)}>
              <span className="checkmark">{shown.includes(view) ? "✓" : ""}</span>
              {viewTitle(view)}
              <span className="kbd">{view === "thinking" ? "Ctrl+J" : view === "moves" ? "Ctrl+B" : ""}</span>
            </button>
          ))}
          <hr />
          {Object.entries(PRESETS).map(([k, p]) => (
            <button key={k} onClick={() => setDock(index, p.make())}>{t("レイアウト: {name}", { name: t(p.title) })}</button>
          ))}
          <button onClick={() => applyDockToAll(index)}>{t("この配置を全ビューに適用")}</button>
        </Menu>
      </div>

      <div className="pane-main">
        <Dock layout={dock} onChange={(l) => setDock(index, l)} renderView={renderView} />
      </div>

      {dialog === "game" && <NewGameDialog v={v} onClose={() => setDialog(null)} />}
      {dialog === "analysis" && <AnalysisDialog v={v} onClose={() => setDialog(null)} />}
      {dialog === "paste" && (
        <TextDialog title={t("棋譜・局面の読み込み（KIF / KI2 / CSA / SFEN / USI position）")} initial="" onClose={() => setDialog(null)}
          onSubmit={async (text) => { if (await runOk(api.importText(id, text))) toast(t("読み込みました")); }} />
      )}
      {dialog && typeof dialog === "object" && (
        <TextDialog title={dialog.title} initial={dialog.text} readOnly onClose={() => setDialog(null)} />
      )}
    </div>
  );
}
