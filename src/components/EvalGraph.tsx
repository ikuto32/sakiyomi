import { useMemo, useRef, useState } from "react";
import type { Eval, NodeAnalysis, SessionView } from "../api";
import { currentLine, formatEval } from "../shogi";
import { t } from "../i18n";

const W = 600;
const H = 120;
const PAD = 6;

/** Mate scores are mapped near ±30000; they do not set the scale. */
const MATE_THRESHOLD = 20000;

type SeriesKey = keyof NodeAnalysis;
const SERIES: { key: SeriesKey; mark: string; fallback: string }[] = [
  { key: "black", mark: "☗", fallback: "先手" },
  { key: "white", mark: "☖", fallback: "後手" },
  { key: "analysis", mark: "◎", fallback: "検討" },
];

interface Point { i: number; node: number; e: Eval }

/**
 * Evaluation graph from Sente's point of view. Every engine gets its own line: the engine
 * playing Sente, the one playing Gote (their readings when they chose each move), and the
 * analysis engine. Click a chip in the legend to hide or show a line.
 */
export function EvalGraph({ v, live, onSelect }: { v: SessionView; live: { node: number; eval: Eval } | null; onSelect: (n: number) => void }) {
  const ref = useRef<SVGSVGElement>(null);
  const [hidden, setHidden] = useState<Record<SeriesKey, boolean>>({ black: false, white: false, analysis: false });
  const line = useMemo(() => currentLine(v.nodes, v.cursor), [v.nodes, v.cursor]);
  const n = Math.max(line.length - 1, 1);
  const xOf = (i: number) => PAD + (i / n) * (W - 2 * PAD);

  const series = useMemo(() => SERIES.map((s) => {
    let engine: string | null = null;
    const points: Point[] = [];
    line.forEach((node, i) => {
      const r = v.nodes[node].analysis[s.key];
      engine ??= r?.engine ?? null;
      const e = s.key === "analysis" && live && live.node === node ? live.eval : r?.eval;
      if (e) points.push({ i, node, e });
    });
    return { ...s, engine, points };
  }), [line, v.nodes, live]);

  // The scale follows the game: quiet games stay readable, big swings and mates still fit
  // (tanh compresses everything beyond the range smoothly).
  const range = useMemo(() => {
    let max = 0;
    for (const s of series) for (const p of s.points) if (Math.abs(p.e.cp) < MATE_THRESHOLD) max = Math.max(max, Math.abs(p.e.cp));
    return Math.min(1200, Math.max(300, Math.ceil(max / 100) * 100));
  }, [series]);
  const yOf = (cp: number) => H / 2 - Math.tanh(cp / range) * (H / 2 - PAD);
  const grid = Math.max(100, Math.round(range / 200) * 100);

  const pathOf = (pts: Point[]) => pts.map((p, k) => `${k ? "L" : "M"}${xOf(p.i).toFixed(1)},${yOf(p.e.cp).toFixed(1)}`).join(" ");
  const analysis = series[2];
  const area = analysis.points.length && !hidden.analysis
    ? `M${xOf(analysis.points[0].i)},${H / 2} ` + analysis.points.map((p) => `L${xOf(p.i).toFixed(1)},${yOf(p.e.cp).toFixed(1)}`).join(" ") + ` L${xOf(analysis.points[analysis.points.length - 1].i)},${H / 2} Z`
    : "";
  const ci = line.indexOf(v.cursor);
  const present = series.filter((s) => s.points.length > 0);

  const onClick = (e: React.MouseEvent) => {
    const svg = ref.current;
    if (!svg) return;
    const r = svg.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * W;
    const i = Math.round(((x - PAD) / (W - 2 * PAD)) * n);
    const node = line[Math.max(0, Math.min(line.length - 1, i))];
    if (node !== undefined) onSelect(node);
  };

  return (
    <div className="evalgraph">
      <div className="evalgraph-head">
        <span>{t("評価値グラフ（先手視点）")}</span>
        <span className="eg-legend">
          {present.map((s) => {
            const cur = s.points.find((p) => p.node === v.cursor);
            return (
              <button
                key={s.key}
                className={`eg-chip eg-${s.key}` + (hidden[s.key] ? " off" : "")}
                onClick={() => setHidden({ ...hidden, [s.key]: !hidden[s.key] })}
                title={t(hidden[s.key] ? "クリックで表示" : "クリックで非表示")}
              >
                <span className="eg-swatch" />
                {s.mark} {s.engine ?? t(s.fallback)}
                {cur && <strong>{formatEval(cur.e)}</strong>}
              </button>
            );
          })}
        </span>
      </div>
      <svg ref={ref} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" onClick={onClick} role="img" aria-label={t("評価値グラフ（先手視点）")}>
        <rect x={0} y={0} width={W} height={H} className="eg-bg" />
        {[grid, -grid].map((c) => <line key={c} x1={0} x2={W} y1={yOf(c)} y2={yOf(c)} className="eg-grid" />)}
        <line x1={0} x2={W} y1={H / 2} y2={H / 2} className="eg-axis" />
        {area && <path d={area} className="eg-area" />}
        {series.map((s) => !hidden[s.key] && s.points.length > 0 && (
          <path key={s.key} d={pathOf(s.points)} className={`eg-line eg-${s.key}`} vectorEffect="non-scaling-stroke" />
        ))}
        {ci >= 0 && <line x1={xOf(ci)} x2={xOf(ci)} y1={0} y2={H} className="eg-cursor" vectorEffect="non-scaling-stroke" />}
      </svg>
    </div>
  );
}
