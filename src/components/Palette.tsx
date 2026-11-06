import { useEffect, useMemo, useRef, useState } from "react";
import type { Command } from "./Pane";
import { t } from "../i18n";

/** VS Code-style command palette (Ctrl+Shift+P / F1). */
export function CommandPalette({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const items = useMemo(() => {
    const words = q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    return commands.filter((c) => words.every((w) => c.title.toLowerCase().includes(w)));
  }, [q, commands]);

  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    (listRef.current?.children[sel] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const runAt = (i: number) => {
    const c = items[i];
    onClose();
    if (c) setTimeout(c.run, 0);
  };

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="command-palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          placeholder={t("コマンドを入力（例: 表示 読み筋 / レイアウト / 対局）")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(items.length - 1, s + 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(0, s - 1)); }
            else if (e.key === "Enter") { e.preventDefault(); runAt(sel); }
            else if (e.key === "Escape") { e.preventDefault(); onClose(); }
          }}
        />
        <div className="palette-list" ref={listRef}>
          {items.map((c, i) => (
            <div key={c.id} className={"palette-item" + (i === sel ? " on" : "")} onMouseEnter={() => setSel(i)} onClick={() => runAt(i)}>
              <span>{c.title}</span>
              {c.keys && <span className="kbd">{c.keys}</span>}
            </div>
          ))}
          {!items.length && <div className="palette-item muted">{t("一致するコマンドはありません")}</div>}
        </div>
      </div>
    </div>
  );
}
