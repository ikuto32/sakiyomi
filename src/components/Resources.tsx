import type { SessionView, SystemStats } from "../api";
import { useStore } from "../store";
import { sessionTitle, t } from "../i18n";

const gb = (b: number) => (b / 1024 ** 3).toFixed(1);
const pct = (x: number) => `${Math.round(x)}%`;

function Spark({ data, cls }: { data: number[]; cls: string }) {
  const W = 120;
  const H = 30;
  const n = Math.max(data.length, 2);
  const pts = data.map((v, i) => `${((i + (120 - n)) / 119) * W},${H - (Math.min(100, Math.max(0, v)) / 100) * H}`).join(" ");
  return (
    <svg className={"spark " + cls} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
      <polyline points={pts} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Meter({ label, value, sub, data, cls }: { label: string; value: string; sub?: string; data: number[]; cls: string }) {
  return (
    <div className="meter">
      <div className="meter-head">
        <span>{label}</span>
        <strong>{value}</strong>
      </div>
      <Spark data={data} cls={cls} />
      {sub && <div className="meter-sub">{sub}</div>}
    </div>
  );
}

/** Machine and per-engine resource usage (CPU, memory, GPU). */
export function ResourcesView({ v }: { v: SessionView }) {
  const s = useStore((st) => st.system);
  const history = useStore((st) => st.history);
  const sessions = useStore((st) => st.sessions);
  if (!s) return <div className="resources muted">{t("計測中…")}</div>;
  const g = s.gpus[0];
  return (
    <div className="resources">
      <div className="meters">
        <Meter label={t("CPU（{n}論理コア）", { n: s.cores })} value={pct(s.cpu)} sub={t("メモリ {used} / {total} GB", { used: gb(s.memUsed), total: gb(s.memTotal) })} data={history.cpu} cls="cpu" />
        <Meter
          label={g ? g.name : "GPU"}
          value={g ? pct(g.util) : "—"}
          sub={g ? `VRAM ${gb(g.memUsed)} / ${gb(g.memTotal)} GB${g.temp !== null ? `　${g.temp}°C` : ""}${g.powerW !== null ? `　${g.powerW.toFixed(0)} W` : ""}` : s.gpuError ? t("NVMLを利用できません: {e}", { e: s.gpuError }) : t("GPUが見つかりません")}
          data={history.gpu}
          cls="gpu"
        />
        <Meter label={t("このアプリのエンジン（CPU）")} value={pct(history.engineCpu[history.engineCpu.length - 1] ?? 0)} sub={t("{n} プロセス", { n: s.engines.length })} data={history.engineCpu} cls="engine" />
      </div>
      <table className="proc-table">
        <thead>
          <tr><th>{t("エンジン")}</th><th>{t("対局")}</th><th>PID</th><th>CPU</th><th>{t("メモリ")}</th><th>GPU</th><th>VRAM</th></tr>
        </thead>
        <tbody>
          {s.engines.map((e) => (
            <tr key={e.pid} className={e.session === v.id ? "mine" : ""}>
              <td>{e.name}</td>
              <td>{(() => { const x = sessions.get(e.session); return x ? sessionTitle(x) : e.session; })()}</td>
              <td>{e.pid}</td>
              <td>{e.cpu.toFixed(1)}%</td>
              <td>{(e.memBytes / 1024 ** 2).toFixed(0)} MB</td>
              <td>{e.gpuUtil !== null ? `${e.gpuUtil}%` : "—"}</td>
              <td>{e.gpuMem !== null ? `${(e.gpuMem / 1024 ** 2).toFixed(0)} MB` : "—"}</td>
            </tr>
          ))}
          {!s.engines.length && <tr><td colSpan={7} className="muted">{t("起動中のエンジンはありません")}</td></tr>}
        </tbody>
      </table>
    </div>
  );
}

/** VS Code-style status bar. */
export function StatusBar({ onResources }: { onResources: () => void }) {
  const s: SystemStats | null = useStore((st) => st.system);
  const activePane = useStore((st) => st.activePane);
  const pane = useStore((st) => st.panes[st.activePane]);
  const layout = useStore((st) => st.layout);
  const v = useStore((st) => (pane?.sessionId != null ? st.sessions.get(pane.sessionId) : undefined));
  const g = s?.gpus[0];
  const engineCpu = s ? s.engines.reduce((a, e) => a + e.cpu, 0) : 0;
  return (
    <footer className="statusbar">
      <span className="sb-item">{layout > 1 ? t("ビュー {n}: ", { n: activePane + 1 }) : ""}{v ? sessionTitle(v) : ""}</span>
      {v?.status && <span className="sb-item sb-status">{v.status}</span>}
      <span className="spacer" />
      <span className="sb-item sb-hint">{t("Ctrl+Shift+P コマンド")}</span>
      {s && (
        <button className="sb-item" onClick={onResources} title={t("クリックでリソースビューを表示")}>
          <span>CPU {pct(s.cpu)}</span>
          <span>{t("メモリ {used}/{total} GB", { used: gb(s.memUsed), total: gb(s.memTotal) })}</span>
          {g ? <span>GPU {pct(g.util)} · {gb(g.memUsed)}/{gb(g.memTotal)} GB{g.temp !== null ? ` · ${g.temp}°C` : ""}</span> : <span>GPU —</span>}
          <span>{t("エンジン {n} · CPU {cpu}%", { n: s.engines.length, cpu: engineCpu.toFixed(1) })}</span>
        </button>
      )}
    </footer>
  );
}
