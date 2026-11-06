// Conversion and book-mining tools. Everything long-running is a backend job; this dialog only
// collects options, starts jobs and shows their progress. The same operations are available
// from the command line as `sakiyomi-tool`.
import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  api, BookConvertRequest, BookReport, DbConvertRequest, EvalStyle, JobView, KifuConvertRequest, MineRequest,
  MinerProgress, PositionFormat, TextEncoding,
} from "../api";
import { clearFinishedJobs, run, setToolsOpen, toast, useStore } from "../store";
import { t } from "../i18n";
import { Modal } from "./Dialogs";

type Tab = "book" | "mine" | "kifu" | "position" | "db";
const TABS: [Tab, string][] = [
  ["book", "定跡変換"],
  ["mine", "定跡掘り"],
  ["kifu", "棋譜一括変換"],
  ["position", "局面変換"],
  ["db", "棋譜DB変換"],
];

const JOB_KINDS: Record<JobView["kind"], string> = {
  convertBooks: "定跡変換",
  mine: "定跡掘り",
  convertKifu: "棋譜一括変換",
  convertDb: "棋譜DB変換",
  bookStats: "定跡の統計",
  openBook: "定跡を開く",
};

const BOOK_EXT = ["spg", "spgt", "spgm", "db", "sbk", "bin"];
const KIFU_EXT = ["spg", "spgt", "kif", "kifu", "ki2", "ki2u", "csa", "jkf", "sfen", "usi"];
const DB_EXT = ["spg", "spgt", "spgm", "hcpe", "psv", "hcpe3", "sfen", "csa", "txt"];

/** Per-tool form state, remembered between sessions (best effort). */
function useSaved<T extends object>(key: string, initial: T): [T, (patch: Partial<T>) => void] {
  const [v, setV] = useState<T>(() => {
    try {
      return { ...initial, ...JSON.parse(localStorage.getItem(`tool:${key}`) ?? "{}") };
    } catch {
      return initial;
    }
  });
  const update = (patch: Partial<T>) => {
    const next = { ...v, ...patch };
    setV(next);
    try {
      localStorage.setItem(`tool:${key}`, JSON.stringify(next));
    } catch { /* storage unavailable */ }
  };
  return [v, update];
}

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const numOrNull = (s: string) => (s.trim() === "" ? null : Number(s));

function NumField({ label, value, onChange, min, step, hint }: {
  label: string; value: number | null; onChange: (v: number | null) => void; min?: number; step?: number; hint?: string;
}) {
  return (
    <label className="field" title={hint}>
      <span>{label}</span>
      <input type="number" min={min} step={step} value={value ?? ""} placeholder={t("指定なし")} onChange={(e) => onChange(numOrNull(e.target.value))} />
    </label>
  );
}

function PathField({ label, value, onPick, placeholder }: { label: string; value: string; onPick: () => void; placeholder?: string }) {
  return (
    <div className="field path-field">
      <span>{label}</span>
      <div className="path-row">
        <input readOnly value={value} placeholder={placeholder ?? t("未選択")} title={value} />
        <button onClick={onPick}>{t("参照…")}</button>
      </div>
    </div>
  );
}

export function reportText(r: BookReport): string {
  const parts = [t("{p}局面・{m}手", { p: r.positions, m: r.moves })];
  if (r.illegalMoves) parts.push(t("不正な手 {n} を除外", { n: r.illegalMoves }));
  if (r.invalidPositions) parts.push(t("不正な局面 {n} を除外", { n: r.invalidPositions }));
  if (r.duplicatePositions) parts.push(t("重複局面 {n} を統合", { n: r.duplicatePositions }));
  if (r.unreachable) parts.push(t("到達できないエントリ {n}", { n: r.unreachable }));
  if (r.needsRoots) parts.push(t("初期局面から到達できない局面 {n}（読み戻すには起点の定跡が必要）", { n: r.needsRoots }));
  if (r.lostFields.length) parts.push(t("出力形式に保存できない項目: {f}", { f: r.lostFields.join(", ") }));
  return parts.join(" / ");
}

function jobSummary(j: JobView): string {
  if (j.error) return j.error;
  if (!j.done) {
    if (j.kind === "mine" && j.message.startsWith("{")) {
      try {
        const p = JSON.parse(j.message) as MinerProgress;
        return t("ラウンド {r}・探索 {s}・待ち {q}・局面 {p}・エラー {e}", { r: p.round, s: p.searched, q: p.queued, p: p.positions, e: p.errors });
      } catch { /* fall through */ }
    }
    return j.message || t("実行中…");
  }
  const r = j.result as Record<string, unknown> | null;
  if (!r) return t("完了");
  switch (j.kind) {
    case "convertBooks":
      return reportText(r as unknown as BookReport);
    case "bookStats": {
      const s = r.stats as Record<string, number>;
      return reportText(r.report as BookReport) + " — " + t("先手番 {b}・評価値付き {v}・最大手数 {m}", { b: s.blackToMove, v: s.withValues, m: s.maxPly });
    }
    case "convertKifu": {
      const k = r as { converted: number; failed: [string, string][] };
      return t("{ok} 件変換・{ng} 件失敗", { ok: k.converted, ng: k.failed.length }) + (k.failed.length ? `: ${k.failed.slice(0, 3).map(([p, e]) => `${baseName(p)} (${e})`).join(", ")}` : "");
    }
    case "convertDb": {
      const d = r as { games: number; positionsWritten: number; skipped: number };
      return t("{g} 局・{p} 局面を出力・{s} 局面をスキップ", { g: d.games, p: d.positionsWritten, s: d.skipped });
    }
    case "mine": {
      const p = r as unknown as MinerProgress;
      return t("{r} ラウンド・{s} 局面を探索・定跡 {p} 局面", { r: p.round, s: p.searched, p: p.positions });
    }
    case "openBook":
      return t("{n} 局面", { n: (r as { positions: number }).positions });
  }
  return t("完了");
}

function Jobs() {
  const jobs = useStore((s) => s.jobs);
  const list = [...jobs.values()].sort((a, b) => b.id - a.id);
  if (!list.length) return <p className="muted small">{t("実行中のジョブはありません。")}</p>;
  return (
    <div className="jobs">
      {list.map((j) => (
        <div key={j.id} className={"job" + (j.error ? " failed" : j.done ? " done" : "")}>
          <div className="job-head">
            <strong>{t(JOB_KINDS[j.kind] ?? j.kind)}</strong>
            <span className="job-title" title={j.title}>{j.title}</span>
            {!j.done && <button className="link" onClick={() => run(api.cancelJob(j.id))}>{t("中止")}</button>}
          </div>
          <div className="job-bar"><div style={{ width: `${Math.round((j.done ? 1 : j.progress ?? 0) * 100)}%` }} className={j.progress === null && !j.done ? "indeterminate" : ""} /></div>
          <div className="job-msg">{jobSummary(j)}</div>
        </div>
      ))}
      <button className="link" onClick={clearFinishedJobs}>{t("完了したジョブを消去")}</button>
    </div>
  );
}

// ------------------------------------------------------------------ book conversion

function BookTool() {
  const [f, set] = useSaved("book", {
    inputs: [] as string[], output: "", outputFormat: "spg" as BookConvertRequest["outputFormat"], merge: "sum" as BookConvertRequest["merge"],
    minCount: null as number | null, maxValueDrop: null as number | null, maxPly: null as number | null, side: "" as "" | "0" | "1",
    topMoves: null as number | null, backUp: false, sbkEvals: true, rootsFrom: "", noProv: false, overwrite: false,
  });
  const spgOut = f.outputFormat === "spg" || f.outputFormat === "spgt" || f.outputFormat === "spgm";
  const addInputs = async () => {
    const p = await open({ multiple: true, filters: [{ name: t("定跡"), extensions: BOOK_EXT }, { name: t("すべて"), extensions: ["*"] }] });
    if (Array.isArray(p)) set({ inputs: [...f.inputs, ...p.filter((x) => !f.inputs.includes(x))] });
  };
  const pickOutput = async () => {
    const ext = f.outputFormat === "gikou" ? "bin" : f.outputFormat;
    const p = await save({ defaultPath: `book.${ext}`, filters: [{ name: t("定跡"), extensions: [ext] }] });
    if (p) set({ output: p });
  };
  const pickRoots = async () => {
    const p = await open({ multiple: false, filters: [{ name: t("定跡"), extensions: BOOK_EXT }] });
    set({ rootsFrom: typeof p === "string" ? p : "" });
  };
  const start = async () => {
    const req: BookConvertRequest = {
      inputs: f.inputs, output: f.output, outputFormat: f.outputFormat, merge: f.merge,
      minCount: f.minCount, maxValueDrop: f.maxValueDrop, maxPly: f.maxPly, side: f.side === "" ? null : (Number(f.side) as 0 | 1),
      topMoves: f.topMoves, backUp: f.backUp, sbkEvalsFromValues: f.sbkEvals, gikouRoots: [], rootsFrom: f.rootsFrom || null, noProv: spgOut && f.noProv, overwrite: f.overwrite,
    };
    if (await run(api.jobConvertBooks(req))) toast(t("変換を開始しました"));
  };
  return (
    <div className="tool">
      <p className="muted small">{t("SPG・YaneuraOu定跡（.db）・ShogiGUI定跡（.sbk）・技巧定跡（book.bin）を相互に変換します。複数の入力はマージされます。")}</p>
      <div className="field">
        <span>{t("入力")}</span>
        <div className="file-list">
          {f.inputs.map((p) => (
            <div key={p} className="file-item" title={p}>
              <span>{baseName(p)}</span>
              <button className="link" onClick={() => set({ inputs: f.inputs.filter((x) => x !== p) })}>{t("除く")}</button>
              <button className="link" onClick={() => run(api.jobBookStats(p))}>{t("統計")}</button>
            </div>
          ))}
          <button onClick={addInputs}>{t("＋ 追加…")}</button>
        </div>
      </div>
      <div className="tool-grid">
        <label className="field"><span>{t("出力形式")}</span>
          <select value={f.outputFormat} onChange={(e) => set({ outputFormat: e.target.value as BookConvertRequest["outputFormat"], output: "" })}>
            <option value="spg">{t("SPG（.spg・標準）")}</option>
            <option value="spgt">{t("SPGテキスト（.spgt）")}</option>
            <option value="spgm">{t("SPGファイル群（.spgm・大規模向け）")}</option>
            <option value="db">{t("YaneuraOu定跡（.db）")}</option>
            <option value="sbk">{t("ShogiGUI定跡（.sbk）")}</option>
            <option value="gikou">{t("技巧定跡（.bin）")}</option>
          </select>
        </label>
        <label className="field"><span>{t("同じ手の統合")}</span>
          <select value={f.merge} onChange={(e) => set({ merge: e.target.value as BookConvertRequest["merge"] })}>
            <option value="sum">{t("出現回数を合算")}</option>
            <option value="keepFirst">{t("先の定跡を優先")}</option>
            <option value="preferSecond">{t("後の定跡を優先")}</option>
          </select>
        </label>
        <NumField label={t("最小出現回数")} value={f.minCount} min={0} onChange={(minCount) => set({ minCount })} />
        <NumField label={t("最善手との評価値差の上限")} value={f.maxValueDrop} min={0} onChange={(maxValueDrop) => set({ maxValueDrop })} />
        <NumField label={t("最大手数")} value={f.maxPly} min={1} onChange={(maxPly) => set({ maxPly })} />
        <NumField label={t("1局面あたりの最大手数")} value={f.topMoves} min={1} onChange={(topMoves) => set({ topMoves })} />
        <label className="field"><span>{t("手番")}</span>
          <select value={f.side} onChange={(e) => set({ side: e.target.value as "" | "0" | "1" })}>
            <option value="">{t("両方")}</option>
            <option value="0">{t("先手番のみ")}</option>
            <option value="1">{t("後手番のみ")}</option>
          </select>
        </label>
      </div>
      <label className="checkbox" title={t("末端の評価値をミニマックスで根元へ伝播します（ペタショック）")}><input type="checkbox" checked={f.backUp} onChange={(e) => set({ backUp: e.target.checked })} /> {t("評価値を伝播する（ペタショック）")}</label>
      {spgOut && f.inputs.length > 1 && (
        <label className="checkbox" title={t("SPGでは、複数の定跡を統合するとき、各局面・各手にどの定跡から来たか（とその定跡での値）を由来情報（PROV）として記録します。記録しないとファイルは小さくなりますが、出典ごとの値を取り出せなくなります。")}>
          <input type="checkbox" checked={f.noProv} onChange={(e) => set({ noProv: e.target.checked })} /> {t("由来情報（PROV）を書かない")}
        </label>
      )}
      {f.outputFormat === "sbk" && <label className="checkbox"><input type="checkbox" checked={f.sbkEvals} onChange={(e) => set({ sbkEvals: e.target.checked })} /> {t("評価値を局面の評価として保存（.sbk）")}</label>}
      {f.inputs.some((p) => p.toLowerCase().endsWith(".bin")) && (
        <PathField label={t("技巧定跡の起点（任意の定跡）")} value={f.rootsFrom} onPick={pickRoots} placeholder={t("初期局面から辿れる局面のみ読み込みます")} />
      )}
      <PathField label={t("出力先")} value={f.output} onPick={pickOutput} />
      <label className="checkbox"><input type="checkbox" checked={f.overwrite} onChange={(e) => set({ overwrite: e.target.checked })} /> {t("既存のファイルを上書きする")}</label>
      <div className="dialog-actions"><span className="spacer" /><button className="primary" disabled={!f.inputs.length || !f.output} onClick={start}>{t("変換")}</button></div>
    </div>
  );
}

// ------------------------------------------------------------------ book mining

function MineTool() {
  const engines = useStore((s) => s.engines);
  const [f, set] = useSaved("mine", {
    engineId: "", rawOut: "", bookOut: "", workers: 1, nodes: 1_000_000, multipv: 4, evalDiff: 30, maxPly: 200,
    leavesPerRound: 64, rounds: 0, black: true, white: true, roots: "",
  });
  const engineId = engines.some((e) => e.id === f.engineId) ? f.engineId : engines[0]?.id ?? "";
  const start = async () => {
    const roots: string[] = [];
    for (const line of f.roots.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)) {
      const sfen = await run(api.convertPosition(line, null, "sfen"));
      if (sfen === undefined) return;
      roots.push(sfen.replace(/^sfen /, ""));
    }
    const req: MineRequest = {
      bookIn: f.rawOut, rawOut: f.rawOut, bookOut: f.bookOut || null, workers: f.workers, roots, nodes: f.nodes,
      multipv: f.multipv, multipvDelta: 100, evalDiff: f.evalDiff, maxPly: f.maxPly, leavesPerRound: f.leavesPerRound,
      rounds: f.rounds, black: f.black, white: f.white,
    };
    if (await run(api.jobMine(engineId, req))) toast(t("定跡掘りを開始しました"));
  };
  const pick = async (key: "rawOut" | "bookOut") => {
    const p = await save({ defaultPath: key === "rawOut" ? "mined_raw.db" : "mined_book.db", filters: [{ name: t("定跡"), extensions: ["db"] }] });
    if (p) set({ [key]: p });
  };
  return (
    <div className="tool">
      <p className="muted small">{t("BookMiner方式でエンジンに定跡を掘らせます。末端局面をMultiPVで探索し、評価値を伝播して次に掘る局面を選びます。生の定跡はラウンドごとに保存され、同じファイルを指定すると続きから再開します。")}</p>
      <div className="tool-grid">
        <label className="field"><span>{t("エンジン")}</span>
          <select value={engineId} onChange={(e) => set({ engineId: e.target.value })}>
            {engines.map((e) => <option key={e.id} value={e.id}>{e.name}</option>)}
          </select>
        </label>
        <NumField label={t("並列数（エンジンの数）")} value={f.workers} min={1} onChange={(v) => set({ workers: Math.max(1, v ?? 1) })} hint={t("各エンジンのThreadsは「エンジン管理」で設定します")} />
        <NumField label={t("1局面のノード数")} value={f.nodes} min={1000} step={100000} onChange={(v) => set({ nodes: v ?? 1_000_000 })} />
        <NumField label={t("初期MultiPV")} value={f.multipv} min={1} onChange={(v) => set({ multipv: v ?? 4 })} />
        <NumField label={t("相手の手を広げる評価値差")} value={f.evalDiff} min={0} onChange={(v) => set({ evalDiff: v ?? 30 })} />
        <NumField label={t("最大手数")} value={f.maxPly} min={1} onChange={(v) => set({ maxPly: v ?? 200 })} />
        <NumField label={t("1ラウンドの局面数")} value={f.leavesPerRound} min={1} onChange={(v) => set({ leavesPerRound: v ?? 64 })} />
        <NumField label={t("ラウンド数（0 = 無制限）")} value={f.rounds} min={0} onChange={(v) => set({ rounds: v ?? 0 })} />
      </div>
      <div className="radio-row">
        <label><input type="checkbox" checked={f.black} onChange={(e) => set({ black: e.target.checked })} /> {t("先手番の定跡")}</label>
        <label><input type="checkbox" checked={f.white} onChange={(e) => set({ white: e.target.checked })} /> {t("後手番の定跡")}</label>
      </div>
      <label className="field"><span>{t("起点の局面（1行に1つ、SFEN／position。空なら初期局面）")}</span>
        <textarea className="roots" value={f.roots} onChange={(e) => set({ roots: e.target.value })} placeholder="position startpos moves 7g7f 3c3d" />
      </label>
      <PathField label={t("生の定跡（保存・再開用）")} value={f.rawOut} onPick={() => pick("rawOut")} />
      <PathField label={t("対局用の定跡（評価値を伝播、任意）")} value={f.bookOut} onPick={() => pick("bookOut")} />
      <div className="dialog-actions">
        <span className="muted small">{t("中止しても、それまでの結果は保存されます。")}</span>
        <span className="spacer" />
        <button className="primary" disabled={!engineId || !f.rawOut} onClick={start}>{t("開始")}</button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ batch kifu conversion

function KifuTool() {
  const [f, set] = useSaved("kifu", {
    inputs: [] as string[], outDir: "", format: "kif" as KifuConvertRequest["format"], encoding: "sjis" as TextEncoding,
    evalStyle: "shogigui" as EvalStyle, overwrite: false,
  });
  const add = async () => {
    const p = await open({ multiple: true, filters: [{ name: t("棋譜"), extensions: KIFU_EXT }, { name: t("すべて"), extensions: ["*"] }] });
    if (Array.isArray(p)) set({ inputs: [...f.inputs, ...p.filter((x) => !f.inputs.includes(x))] });
  };
  const pickDir = async () => {
    const p = await open({ directory: true, multiple: false });
    if (typeof p === "string") set({ outDir: p });
  };
  const start = async () => {
    const req: KifuConvertRequest = { inputs: f.inputs, outDir: f.outDir, format: f.format, encoding: f.encoding, evalStyle: f.evalStyle, overwrite: f.overwrite };
    if (await run(api.jobConvertKifu(req))) toast(t("変換を開始しました"));
  };
  return (
    <div className="tool">
      <p className="muted small">{t("SPG・KIF・KI2・CSA・JKF・USIの棋譜をまとめて変換します。評価値と読み筋のコメントは、どの形式からでも読み取り、指定した形式で書き出します。")}</p>
      <div className="field">
        <span>{t("入力（{n} 件）", { n: f.inputs.length })}</span>
        <div className="file-list">
          {f.inputs.slice(0, 50).map((p) => (
            <div key={p} className="file-item" title={p}><span>{baseName(p)}</span><button className="link" onClick={() => set({ inputs: f.inputs.filter((x) => x !== p) })}>{t("除く")}</button></div>
          ))}
          {f.inputs.length > 50 && <div className="muted small">{t("ほか {n} 件", { n: f.inputs.length - 50 })}</div>}
          <div className="path-row"><button onClick={add}>{t("＋ 追加…")}</button>{f.inputs.length > 0 && <button className="link" onClick={() => set({ inputs: [] })}>{t("すべて除く")}</button>}</div>
        </div>
      </div>
      <div className="tool-grid">
        <label className="field"><span>{t("出力形式")}</span>
          <select value={f.format} onChange={(e) => set({ format: e.target.value as KifuConvertRequest["format"] })}>
            <option value="spg">{t("SPG（標準）")}</option><option value="spgt">{t("SPGテキスト")}</option>
            <option value="kif">KIF</option><option value="ki2">KI2</option><option value="csa">CSA</option><option value="jkf">JKF (JSON)</option><option value="usi">USI (position)</option>
          </select>
        </label>
        <label className="field"><span>{t("文字コード")}</span>
          <select value={f.encoding} onChange={(e) => set({ encoding: e.target.value as TextEncoding })}>
            <option value="sjis">Shift_JIS</option><option value="utf8">UTF-8</option><option value="utf8bom">UTF-8 (BOM)</option>
          </select>
        </label>
        <label className="field"><span>{t("評価値・読み筋")}</span>
          <select value={f.evalStyle} onChange={(e) => set({ evalStyle: e.target.value as EvalStyle })}>
            <option value="shogigui">ShogiGUI</option><option value="shogihome">ShogiHome</option><option value="floodgate">Floodgate (CSA)</option><option value="none">{t("保存しない")}</option>
          </select>
        </label>
      </div>
      <PathField label={t("出力フォルダ")} value={f.outDir} onPick={pickDir} />
      <label className="checkbox"><input type="checkbox" checked={f.overwrite} onChange={(e) => set({ overwrite: e.target.checked })} /> {t("既存のファイルを上書きする")}</label>
      <div className="dialog-actions"><span className="spacer" /><button className="primary" disabled={!f.inputs.length || !f.outDir} onClick={start}>{t("変換")}</button></div>
    </div>
  );
}

// ------------------------------------------------------------------ single positions

const POSITION_FORMATS: [PositionFormat, string][] = [
  ["sfen", "SFEN"], ["usi", "USI position"], ["bod", "BOD（局面図）"], ["csa", "CSA"], ["hcp", "HCP（cshogi, 16進）"], ["psfen", "PackedSfen（16進）"],
];

function PositionTool() {
  const [input, setInput] = useState("");
  const [from, setFrom] = useState<PositionFormat | "">("");
  const [to, setTo] = useState<PositionFormat>("bod");
  const [output, setOutput] = useState("");
  const convert = async () => {
    const r = await run(api.convertPosition(input, from || null, to));
    if (r !== undefined) setOutput(r);
  };
  return (
    <div className="tool">
      <p className="muted small">{t("SFEN・USI position・BOD・CSA・HCP・PackedSfenを相互に変換します。")}</p>
      <div className="tool-grid">
        <label className="field"><span>{t("入力形式")}</span>
          <select value={from} onChange={(e) => setFrom(e.target.value as PositionFormat | "")}>
            <option value="">{t("自動判別")}</option>
            {POSITION_FORMATS.map(([k, l]) => <option key={k} value={k}>{t(l)}</option>)}
          </select>
        </label>
        <label className="field"><span>{t("出力形式")}</span>
          <select value={to} onChange={(e) => setTo(e.target.value as PositionFormat)}>
            {POSITION_FORMATS.map(([k, l]) => <option key={k} value={k}>{t(l)}</option>)}
          </select>
        </label>
      </div>
      <textarea className="text-io small" value={input} onChange={(e) => setInput(e.target.value)} placeholder="lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1" />
      <div className="path-row"><button className="primary" disabled={!input.trim()} onClick={convert}>{t("変換")}</button>
        <button disabled={!output} onClick={async () => { await writeText(output); toast(t("コピーしました")); }}>{t("結果をコピー")}</button></div>
      <textarea className="text-io small mono" readOnly value={output} />
    </div>
  );
}

// ------------------------------------------------------------------ databases

function DbTool() {
  const [f, set] = useSaved("db", {
    input: "", output: "", outputFormat: "hcpe" as DbConvertRequest["outputFormat"], skipWithoutEval: false, minPly: 0, overwrite: false,
  });
  const pickIn = async () => {
    const p = await open({ multiple: false, filters: [{ name: t("棋譜DB"), extensions: DB_EXT }, { name: t("すべて"), extensions: ["*"] }] });
    if (typeof p === "string") set({ input: p });
  };
  const pickOut = async () => {
    const ext = f.outputFormat === "sfen" ? "sfen" : f.outputFormat;
    const p = await save({ defaultPath: `out.${ext}`, filters: [{ name: t("棋譜DB"), extensions: [ext] }] });
    if (p) set({ output: p });
  };
  const start = async () => {
    const req: DbConvertRequest = { input: f.input, output: f.output, outputFormat: f.outputFormat, skipWithoutEval: f.skipWithoutEval, minPly: f.minPly, overwrite: f.overwrite };
    if (await run(api.jobConvertDb(req))) toast(t("変換を開始しました"));
  };
  return (
    <div className="tool">
      <p className="muted small">{t("学習・研究用の棋譜データベースを変換します。hcpe・psvは局面単位、SPG・hcpe3・SFEN棋譜・CSA複数棋譜は対局単位の形式です。")}</p>
      <PathField label={t("入力")} value={f.input} onPick={pickIn} />
      <div className="tool-grid">
        <label className="field"><span>{t("出力形式")}</span>
          <select value={f.outputFormat} onChange={(e) => set({ outputFormat: e.target.value as DbConvertRequest["outputFormat"], output: "" })}>
            <option value="spg">{t("SPG（標準）")}</option><option value="spgt">{t("SPGテキスト")}</option>
            <option value="hcpe">hcpe (cshogi)</option><option value="psv">PackedSfenValue (.bin)</option><option value="hcpe3">hcpe3 (DeepLearningShogi)</option>
            <option value="sfen">{t("SFEN棋譜（1行1局）")}</option><option value="csa">{t("CSA複数棋譜")}</option>
          </select>
        </label>
        <NumField label={t("この手数より前を除く")} value={f.minPly} min={0} onChange={(v) => set({ minPly: v ?? 0 })} />
      </div>
      <label className="checkbox"><input type="checkbox" checked={f.skipWithoutEval} onChange={(e) => set({ skipWithoutEval: e.target.checked })} /> {t("評価値のない局面を除く")}</label>
      <PathField label={t("出力先")} value={f.output} onPick={pickOut} />
      <label className="checkbox"><input type="checkbox" checked={f.overwrite} onChange={(e) => set({ overwrite: e.target.checked })} /> {t("既存のファイルを上書きする")}</label>
      <div className="dialog-actions"><span className="spacer" /><button className="primary" disabled={!f.input || !f.output} onClick={start}>{t("変換")}</button></div>
    </div>
  );
}

export function ToolsDialog() {
  const [tab, setTab] = useState<Tab>(() => (localStorage.getItem("toolTab") as Tab | null) ?? "book");
  useEffect(() => {
    try { localStorage.setItem("toolTab", tab); } catch { /* ignore */ }
  }, [tab]);
  return (
    <Modal title={t("ツール")} onClose={() => setToolsOpen(false)} wide>
      <div className="tools">
        <nav className="tools-nav">
          {TABS.map(([k, label]) => <button key={k} className={tab === k ? "on" : ""} onClick={() => setTab(k)}>{t(label)}</button>)}
          <h4>{t("ジョブ")}</h4>
          <Jobs />
        </nav>
        <div className="tools-body">
          {tab === "book" && <BookTool />}
          {tab === "mine" && <MineTool />}
          {tab === "kifu" && <KifuTool />}
          {tab === "position" && <PositionTool />}
          {tab === "db" && <DbTool />}
        </div>
      </div>
    </Modal>
  );
}
