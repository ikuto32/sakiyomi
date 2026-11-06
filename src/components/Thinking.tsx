import { useEffect, useState } from "react";
import type { GameView, SessionView, ThinkingView } from "../api";
import { formatClock, formatCount, formatEval } from "../shogi";
import { playerName, pvText, t } from "../i18n";

const ROLE_LABEL: Record<ThinkingView["role"], string> = { black: "☗ 先手", white: "☖ 後手", analysis: "検討" };
const roleLabel = (r: ThinkingView["role"]) => t(ROLE_LABEL[r]);

export function ThinkingPanel({ v, views }: { v: SessionView; views: ThinkingView[] }) {
  if (!views.length) {
    return <div className="thinking empty">{t("エンジンの読み筋はここに表示されます")}</div>;
  }
  return (
    <div className="thinking">
      {views.map((th) => {
        const first = th.lines[0];
        const nodeLabel = th.role !== "analysis" || th.node === v.cursor ? "" : t("（{n}手目の局面）", { n: v.nodes[th.node]?.ply ?? "?" });
        return (
          <div key={th.key} className="thinking-engine">
            <div className="thinking-head">
              <span className={"role role-" + th.role}>{roleLabel(th.role)}</span>
              <span className="engine-name">{th.engine}</span>
              {th.searching && <span className="pulse" title={t("思考中")} />}
              <span className="stats">
                {first &&
                  [
                    t("深さ {d}", { d: (first.depth ?? "—") + (first.seldepth ? `/${first.seldepth}` : "") }),
                    t("ノード {n}", { n: formatCount(first.nodes) }),
                    t("NPS {n}", { n: formatCount(first.nps) }),
                    first.timeMs !== null ? t("{s}秒", { s: (first.timeMs / 1000).toFixed(1) }) : "",
                    first.hashfull !== null ? t("ハッシュ {n}%", { n: (first.hashfull / 10).toFixed(0) }) : "",
                  ].filter(Boolean).join("　")}
              </span>
              <span className="node-note">{nodeLabel}</span>
            </div>
            <table className="pv-table">
              <thead>
                <tr><th>#</th><th>{t("評価値")}</th><th>{t("深さ")}</th><th>{t("読み筋")}</th></tr>
              </thead>
              <tbody>
                {th.lines.map((l) => (
                  <tr key={l.multipv}>
                    <td className="mpv">{l.multipv}</td>
                    <td className={"score " + ((l.eval?.cp ?? 0) >= 0 ? "pos" : "neg")}>
                      {formatEval(l.eval)}
                      {l.bound === "lowerbound" ? "↑" : l.bound === "upperbound" ? "↓" : ""}
                    </td>
                    <td className="depth">{l.depth ?? ""}</td>
                    <td className="pv" title={l.pvUsi.join(" ")}>{pvText(l)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      })}
    </div>
  );
}

/** Player cards with live clocks; ticks locally between backend snapshots. */
export function Clocks({ v, game, flipped }: { v: SessionView; game: GameView; flipped: boolean }) {
  const [receivedAt, setReceivedAt] = useState(() => performance.now());
  const [, setTick] = useState(0);
  useEffect(() => setReceivedAt(performance.now()), [game]);
  useEffect(() => {
    if (game.running === null) return;
    const t = setInterval(() => setTick((x) => x + 1), 100);
    return () => clearInterval(t);
  }, [game.running]);

  const now = performance.now();
  const order = flipped ? [0, 1] : [1, 0];
  return (
    <div className="clocks">
      {order.map((side) => {
        const tc = game.times[side];
        const running = game.running === side;
        const spent = running ? game.elapsedMs + (now - receivedAt) : 0;
        const main = game.remainingMs[side] - spent;
        const unlimited = !tc.mainMs && !tc.byoyomiMs && !tc.incrementMs;
        let display = unlimited ? "∞" : formatClock(Math.max(main, 0));
        let byo = "";
        if (!unlimited && tc.byoyomiMs) {
          const left = main >= 0 ? tc.byoyomiMs : tc.byoyomiMs + main;
          byo = t("秒読み {n}", { n: Math.max(0, Math.ceil(left / 1000)) });
          if (main < 0) display = formatClock(0);
        }
        const name = game.players[side].name || playerName(v, side);
        const low = !unlimited && running && main < 10000 && (main < 0 || !tc.byoyomiMs);
        return (
          <div key={side} className={"clock" + (running ? " running" : "") + (low ? " low" : "")}>
            <div className="clock-name">
              <span className="mark">{side === 0 ? "☗" : "☖"}</span>
              <span className="pname">{name}</span>
              <span className="pkind">{game.players[side].kind === "engine" ? t("エンジン") : t("人間")}</span>
            </div>
            <div className="clock-time">
              <span>{display}</span>
              {byo && <span className="byo">{byo}</span>}
            </div>
          </div>
        );
      })}
      {game.over && game.result && <div className="result">{game.result}</div>}
    </div>
  );
}
