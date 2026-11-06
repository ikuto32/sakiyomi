import { useEffect, useMemo, useRef, useState } from "react";
import type { NodeView, SessionView } from "../api";
import { api } from "../api";
import { run } from "../store";
import { currentLine } from "../shogi";
import { moveLabel, t } from "../i18n";

function fmtTime(ms: number | null) {
  if (ms === null) return "";
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function Row({ v, n, active, onSelect, indent = 0 }: { v: SessionView; n: number; active: boolean; onSelect: (n: number) => void; indent?: number }) {
  const node: NodeView = v.nodes[n];
  const branched = node.parent !== null && v.nodes[node.parent].children.length > 1;
  return (
    <div
      className={"move-row" + (active ? " active" : "") + (node.special ? " special" : "")}
      style={indent ? { paddingLeft: 8 + indent * 14 } : undefined}
      onClick={() => onSelect(n)}
      data-node={n}
    >
      <span className="ply">{n === 0 ? "" : node.ply}</span>
      <span className="label">{moveLabel(node)}</span>
      <span className="flags">
        {branched && <span className="branch-mark" title={t("変化あり")}>+</span>}
        {node.comment && <span className="comment-mark" title={node.comment}>✎</span>}
      </span>
      <span className="time">{fmtTime(node.timeMs)}</span>
    </div>
  );
}

function useScrollToCursor(v: SessionView, dep: unknown) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current?.querySelector(`[data-node="${v.cursor}"]`) as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest" });
  }, [v.cursor, dep]);
  return ref;
}

/** The line through the cursor, plus the variation selector and edit actions. */
export function MoveLineView({ v, onSelect }: { v: SessionView; onSelect: (n: number) => void }) {
  const line = useMemo(() => currentLine(v.nodes, v.cursor), [v.nodes, v.cursor]);
  const listRef = useScrollToCursor(v, line.length);
  const cur = v.nodes[v.cursor];
  const siblings = cur.parent !== null ? v.nodes[cur.parent].children : [];
  const locked = !!v.game && !v.game.over;

  return (
    <div className="movelist">
      <div className="moves" ref={listRef}>
        {line.map((n) => <Row key={n} v={v} n={n} active={n === v.cursor} onSelect={onSelect} />)}
      </div>
      {siblings.length > 1 && (
        <div className="variations">
          <div className="variations-title">{t("変化 ({i}/{n}) ↑↓", { i: siblings.indexOf(v.cursor) + 1, n: siblings.length })}</div>
          <div className="variation-list">
            {siblings.map((s, i) => (
              <button key={s} className={s === v.cursor ? "on" : ""} onClick={() => onSelect(s)}>
                {i === 0 ? t("本譜 ") : ""}{moveLabel(v.nodes[s])}
              </button>
            ))}
          </div>
        </div>
      )}
      <div className="node-actions">
        <button disabled={locked || v.cursor === 0 || siblings[0] === v.cursor} onClick={() => run(api.promoteVariation(v.id, v.cursor))}>
          {t("本譜にする")}
        </button>
        <button disabled={locked || v.cursor === 0} onClick={() => run(api.deleteNode(v.id, v.cursor))}>
          {t("この手以降を削除")}
        </button>
      </div>
    </div>
  );
}

/** Every variation, nested under the move it branches from. */
export function MoveTreeView({ v, onSelect }: { v: SessionView; onSelect: (n: number) => void }) {
  const listRef = useScrollToCursor(v, v.nodes.length);
  const renderTree = (start: number, depth: number): JSX.Element[] => {
    const out: JSX.Element[] = [];
    const chain: number[] = [];
    for (let n: number | undefined = start; n !== undefined; n = v.nodes[n].children[0]) chain.push(n);
    for (const n of chain) {
      out.push(<Row key={n} v={v} n={n} active={n === v.cursor} onSelect={onSelect} indent={depth} />);
      const p = v.nodes[n].parent;
      if (p !== null && v.nodes[p].children[0] === n) {
        for (const alt of v.nodes[p].children.slice(1)) {
          out.push(<div key={`b${alt}`} className="tree-branch">{renderTree(alt, depth + 1)}</div>);
        }
      }
    }
    return out;
  };
  return (
    <div className="movelist">
      <div className="moves" ref={listRef}>{renderTree(0, 0)}</div>
    </div>
  );
}

export function CommentView({ v }: { v: SessionView }) {
  const node = v.nodes[v.cursor];
  const [text, setText] = useState(node.comment);
  const [editingNode, setEditingNode] = useState(v.cursor);
  if (editingNode !== v.cursor) {
    setEditingNode(v.cursor);
    setText(node.comment);
  }
  return (
    <div className="comment-view">
      <textarea
        className="comment"
        placeholder={t("{n}手目のコメント", { n: node.ply })}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => text !== node.comment && run(api.setComment(v.id, v.cursor, text))}
      />
    </div>
  );
}
