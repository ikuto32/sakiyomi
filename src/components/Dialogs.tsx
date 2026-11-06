import { useEffect, useMemo, useState } from "react";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { api, EngineConfig, GameConfig, OptionDef, PlayerConfig, SessionView, TimeControl } from "../api";
import { reloadEngines, run, toast, useStore } from "../store";
import { START_SFEN } from "../shogi";
import { sessionTitle, t } from "../i18n";

export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: React.ReactNode; wide?: boolean }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={"modal" + (wide ? " wide" : "")} role="dialog" aria-label={title}>
        <div className="modal-head">
          <h2>{title}</h2>
          <button className="icon" onClick={onClose} aria-label={t("閉じる")}>✕</button>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ engines

function OptionInput({ o, value, onChange }: { o: OptionDef; value: string; onChange: (v: string) => void }) {
  switch (o.kind) {
    case "check":
      return <input type="checkbox" checked={value === "true"} onChange={(e) => onChange(e.target.checked ? "true" : "false")} />;
    case "spin":
      return (
        <input type="number" value={value} min={o.min ?? undefined} max={o.max ?? undefined}
          onChange={(e) => onChange(e.target.value)} />
      );
    case "combo":
      return (
        <select value={value} onChange={(e) => onChange(e.target.value)}>
          {o.vars.map((x) => <option key={x} value={x}>{x}</option>)}
        </select>
      );
    case "button":
      return <span className="muted">{t("（ボタン）")}</span>;
    default:
      return <input type="text" value={value} onChange={(e) => onChange(e.target.value)} />;
  }
}

export function EngineManager({ onClose }: { onClose: () => void }) {
  const engines = useStore((s) => s.engines);
  const [selected, setSelected] = useState<string | null>(engines[0]?.id ?? null);
  const [busy, setBusy] = useState(false);
  const engine = engines.find((e) => e.id === selected) ?? null;

  const add = async () => {
    const path = await open({ multiple: false, directory: false, filters: [{ name: t("USIエンジン"), extensions: ["exe", "bat", "cmd"] }] });
    if (!path || typeof path !== "string") return;
    setBusy(true);
    const e = await run(api.registerEngine(path));
    setBusy(false);
    if (e) {
      await reloadEngines();
      setSelected(e.id);
      toast(t("{name} を登録しました（オプション {n} 個）", { name: e.name, n: e.options.length }));
    }
  };

  return (
    <Modal title={t("エンジン管理")} onClose={onClose} wide>
      <div className="engine-manager">
        <div className="engine-list">
          {engines.map((e) => (
            <button key={e.id} className={e.id === selected ? "on" : ""} onClick={() => setSelected(e.id)}>
              <strong>{e.name}</strong>
              <small>{e.author}</small>
            </button>
          ))}
          {!engines.length && <p className="muted">{t("USIエンジン（.exe）を追加してください。")}</p>}
          <button className="primary" disabled={busy} onClick={add}>{busy ? t("読み込み中…") : t("＋ エンジンを追加")}</button>
        </div>
        {engine ? <EngineEditor key={engine.id} engine={engine} onRemoved={() => setSelected(null)} /> : <div className="engine-editor muted">{t("エンジンを選択してください")}</div>}
      </div>
    </Modal>
  );
}

function EngineEditor({ engine, onRemoved }: { engine: EngineConfig; onRemoved: () => void }) {
  const initial = useMemo(() => {
    const v: Record<string, string> = {};
    for (const o of engine.options) v[o.name] = engine.values[o.name] ?? o.default;
    return v;
  }, [engine]);
  const [name, setName] = useState(engine.name);
  const [values, setValues] = useState(initial);
  const [filter, setFilter] = useState("");
  const dirty = name !== engine.name || JSON.stringify(values) !== JSON.stringify(initial);

  const save = async () => {
    const r = await run(api.updateEngine(engine.id, name, values));
    if (r) {
      await reloadEngines();
      toast(t("エンジン設定を保存しました"));
    }
  };
  const remove = async () => {
    if (!(await ask(t("{name} の登録を削除しますか？（ファイルは削除されません）", { name: engine.name }), { title: t("エンジン管理"), kind: "warning" }))) return;
    await run(api.removeEngine(engine.id));
    await reloadEngines();
    onRemoved();
  };
  const refresh = async () => {
    if (await run(api.refreshEngine(engine.id))) {
      await reloadEngines();
      toast(t("オプションを再読み込みしました"));
    }
  };

  const shown = engine.options.filter((o) => o.kind !== "button" && o.name.toLowerCase().includes(filter.toLowerCase()));
  return (
    <div className="engine-editor">
      <label className="field">
        <span>{t("表示名")}</span>
        <input value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <div className="engine-meta">
        <div>{t("USI名: {name} / 作者: {author}", { name: engine.usiName, author: engine.author || "—" })}</div>
        <div className="path" title={engine.path}>{engine.path}</div>
      </div>
      <div className="options-head">
        <strong>{t("エンジンオプション")}</strong>
        <input placeholder={t("絞り込み")} value={filter} onChange={(e) => setFilter(e.target.value)} />
      </div>
      <div className="options">
        {shown.map((o) => {
          const changed = values[o.name] !== o.default;
          return (
            <label key={o.name} className={"option" + (changed ? " changed" : "")}>
              <span className="oname" title={t("{kind} 既定値: {def}", { kind: o.kind, def: o.default || t("(空)") }) + (o.min !== null ? t(" 範囲: {min}〜{max}", { min: o.min, max: o.max ?? "" }) : "")}>{o.name}</span>
              <OptionInput o={o} value={values[o.name] ?? ""} onChange={(x) => setValues({ ...values, [o.name]: x })} />
              {changed && (
                <button className="link" onClick={(e) => { e.preventDefault(); setValues({ ...values, [o.name]: o.default }); }}>{t("既定")}</button>
              )}
            </label>
          );
        })}
      </div>
      <div className="dialog-actions">
        <button className="danger" onClick={remove}>{t("登録削除")}</button>
        <button onClick={refresh}>{t("オプション再取得")}</button>
        <span className="spacer" />
        <button className="primary" disabled={!dirty} onClick={save}>{t("保存")}</button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ new game

const HANDICAPS: [string, string][] = [
  ["平手", START_SFEN],
  ["香落ち", "lnsgkgsn1/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["角落ち", "lnsgkgsnl/1r7/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["飛車落ち", "lnsgkgsnl/7b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["飛香落ち", "lnsgkgsn1/7b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["二枚落ち", "lnsgkgsnl/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["四枚落ち", "1nsgkgsn1/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
  ["六枚落ち", "2sgkgs2/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"],
];

interface SideForm {
  who: string; // "human" or engine id
  name: string;
  minutes: number;
  byoyomi: number;
  increment: number;
  nodes: number;
}

const defaultSide = (who: string): SideForm => ({ who, name: "", minutes: 10, byoyomi: 10, increment: 0, nodes: 0 });

function loadPrefs(): { sides: [SideForm, SideForm]; start: string; maxMoves: number } | null {
  try {
    const raw = localStorage.getItem("newGamePrefs");
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

export function NewGameDialog({ v, onClose }: { v: SessionView; onClose: () => void }) {
  const engines = useStore((s) => s.engines);
  const prefs = loadPrefs();
  const fallbackEngine = engines[0]?.id ?? "human";
  const valid = (w: string) => w === "human" || engines.some((e) => e.id === w);
  const [sides, setSides] = useState<[SideForm, SideForm]>(() => {
    const s = prefs?.sides ?? [defaultSide("human"), defaultSide(fallbackEngine)];
    return s.map((x) => ({ ...x, who: valid(x.who) ? x.who : fallbackEngine })) as [SideForm, SideForm];
  });
  const [start, setStart] = useState(prefs?.start ?? "平手");
  const [maxMoves, setMaxMoves] = useState(prefs?.maxMoves ?? 0);
  const [same, setSame] = useState(true);

  const update = (i: number, patch: Partial<SideForm>) => {
    const next = [...sides] as [SideForm, SideForm];
    next[i] = { ...next[i], ...patch };
    if (same && ("minutes" in patch || "byoyomi" in patch || "increment" in patch)) {
      const o = 1 - i;
      next[o] = { ...next[o], minutes: next[i].minutes, byoyomi: next[i].byoyomi, increment: next[i].increment };
    }
    setSides(next);
  };

  const begin = async () => {
    try {
      localStorage.setItem("newGamePrefs", JSON.stringify({ sides, start, maxMoves }));
    } catch { /* storage may be unavailable */ }
    const player = (s: SideForm): PlayerConfig =>
      s.who === "human"
        ? { kind: "human", name: s.name || t("あなた") }
        : { kind: "engine", engineId: s.who, nodes: s.nodes > 0 ? s.nodes : null };
    const time = (s: SideForm): TimeControl => ({
      mainMs: Math.round(s.minutes * 60000),
      byoyomiMs: Math.round(s.byoyomi * 1000),
      incrementMs: Math.round(s.increment * 1000),
    });
    const startSfen = start === "current" ? null : HANDICAPS.find(([n]) => n === start)?.[1] ?? START_SFEN;
    const config: GameConfig = {
      players: [player(sides[0]), player(sides[1])],
      times: [time(sides[0]), time(sides[1])],
      startSfen,
      maxMoves,
    };
    onClose();
    await run(api.newGame(v.id, config));
  };

  const handicap = start !== "平手" && start !== "current";
  const sideTitle = (i: number) => t(handicap ? (i === 0 ? "☗ 下手" : "☖ 上手") : i === 0 ? "☗ 先手" : "☖ 後手");

  return (
    <Modal title={t("対局 — {title}", { title: sessionTitle(v) })} onClose={onClose} wide>
      <div className="newgame">
        {[0, 1].map((i) => {
          const s = sides[i];
          return (
            <fieldset key={i} className="side-form">
              <legend>{sideTitle(i)}</legend>
              <label className="field">
                <span>{t("対局者")}</span>
                <select value={s.who} onChange={(e) => update(i, { who: e.target.value })}>
                  <option value="human">{t("人間")}</option>
                  {engines.map((e) => <option key={e.id} value={e.id}>{e.name}</option>)}
                </select>
              </label>
              {s.who === "human" ? (
                <label className="field"><span>{t("名前")}</span><input value={s.name} placeholder={t("あなた")} onChange={(e) => update(i, { name: e.target.value })} /></label>
              ) : (
                <label className="field" title={t("0 なら持ち時間で思考します")}>
                  <span>{t("ノード数制限")}</span>
                  <input type="number" min={0} step={1000} value={s.nodes} onChange={(e) => update(i, { nodes: Number(e.target.value) })} />
                </label>
              )}
              <div className="time-grid">
                <label className="field"><span>{t("持ち時間（分）")}</span><input type="number" min={0} step={1} value={s.minutes} onChange={(e) => update(i, { minutes: Number(e.target.value) })} /></label>
                <label className="field"><span>{t("秒読み（秒）")}</span><input type="number" min={0} step={1} value={s.byoyomi} disabled={s.increment > 0} onChange={(e) => update(i, { byoyomi: Number(e.target.value) })} /></label>
                <label className="field"><span>{t("加算（秒）")}</span><input type="number" min={0} step={1} value={s.increment} onChange={(e) => update(i, { increment: Number(e.target.value), byoyomi: Number(e.target.value) > 0 ? 0 : s.byoyomi })} /></label>
              </div>
            </fieldset>
          );
        })}
        <div className="newgame-common">
          <label className="checkbox"><input type="checkbox" checked={same} onChange={(e) => setSame(e.target.checked)} /> {t("両者同じ持ち時間")}</label>
          <label className="field">
            <span>{t("開始局面")}</span>
            <select value={start} onChange={(e) => setStart(e.target.value)}>
              <option value="current">{t("現在の局面から（{n}手目）", { n: v.nodes[v.cursor].ply })}</option>
              {HANDICAPS.map(([n]) => <option key={n} value={n}>{t(n)}</option>)}
            </select>
          </label>
          <label className="field" title={t("0 で無制限。到達すると持将棋（引き分け）")}>
            <span>{t("最大手数")}</span>
            <input type="number" min={0} value={maxMoves} onChange={(e) => setMaxMoves(Number(e.target.value))} />
          </label>
          <button className="swap" onClick={() => setSides([sides[1], sides[0]])}>{t("⇄ 先後入れ替え")}</button>
        </div>
      </div>
      <div className="dialog-actions">
        <span className="muted">{t("エンジンのオプション（Threads等）は「エンジン管理」で設定します")}</span>
        <span className="spacer" />
        <button onClick={onClose}>{t("キャンセル")}</button>
        <button className="primary" onClick={begin}>{t("対局開始")}</button>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ analysis

export function AnalysisDialog({ v, onClose }: { v: SessionView; onClose: () => void }) {
  const engines = useStore((s) => s.engines);
  const [engineId, setEngineId] = useState(() => localStorage.getItem("analysisEngine") ?? engines[0]?.id ?? "");
  const [multipv, setMultipv] = useState(() => Number(localStorage.getItem("analysisMultiPV") ?? 3));
  const [mode, setMode] = useState<"infinite" | "batch">("infinite");
  const [seconds, setSeconds] = useState(1);
  const id = engines.some((e) => e.id === engineId) ? engineId : engines[0]?.id ?? "";

  const startIt = async () => {
    try {
      localStorage.setItem("analysisEngine", id);
      localStorage.setItem("analysisMultiPV", String(multipv));
    } catch { /* ignore */ }
    onClose();
    await run(api.startAnalysis(v.id, id, mode === "batch" ? 1 : multipv, mode === "batch" ? Math.round(seconds * 1000) : null));
  };

  return (
    <Modal title={t("検討 — {title}", { title: sessionTitle(v) })} onClose={onClose}>
      {!engines.length && <p className="muted">{t("先に「エンジン管理」でエンジンを登録してください。")}</p>}
      <label className="field">
        <span>{t("エンジン")}</span>
        <select value={id} onChange={(e) => setEngineId(e.target.value)}>
          {engines.map((e) => <option key={e.id} value={e.id}>{e.name}</option>)}
        </select>
      </label>
      <div className="radio-row">
        <label><input type="radio" checked={mode === "infinite"} onChange={() => setMode("infinite")} /> {t("検討（表示中の局面を無制限に読む）")}</label>
        <label><input type="radio" checked={mode === "batch"} onChange={() => setMode("batch")} /> {t("棋譜解析（本譜を1手ずつ）")}</label>
      </div>
      {mode === "infinite" ? (
        <label className="field"><span>{t("候補手の数（MultiPV）")}</span><input type="number" min={1} max={20} value={multipv} onChange={(e) => setMultipv(Number(e.target.value))} /></label>
      ) : (
        <label className="field"><span>{t("1手あたりの秒数")}</span><input type="number" min={0.1} step={0.5} value={seconds} onChange={(e) => setSeconds(Number(e.target.value))} /></label>
      )}
      <p className="muted small">{t("エンジンはこの対局（{title}）専用に起動され、他のビューには影響しません。", { title: sessionTitle(v) })}</p>
      <div className="dialog-actions">
        <span className="spacer" />
        <button onClick={onClose}>{t("キャンセル")}</button>
        <button className="primary" disabled={!id} onClick={startIt}>{t("開始")}</button>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ text import / export

export function TextDialog({ title, initial, readOnly, onSubmit, onClose }: {
  title: string; initial: string; readOnly?: boolean; onSubmit?: (text: string) => void; onClose: () => void;
}) {
  const [text, setText] = useState(initial);
  return (
    <Modal title={title} onClose={onClose} wide>
      <textarea className="text-io" value={text} readOnly={readOnly} onChange={(e) => setText(e.target.value)} autoFocus />
      <div className="dialog-actions">
        <span className="spacer" />
        <button onClick={onClose}>{t("閉じる")}</button>
        {onSubmit && <button className="primary" onClick={() => { onSubmit(text); onClose(); }}>{t("読み込む")}</button>}
      </div>
    </Modal>
  );
}
