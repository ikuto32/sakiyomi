import { Children, Fragment, useRef } from "react";

const MIN_PX = 48;

/**
 * Children laid out along `dir` with draggable sashes between them (VS Code-style).
 * Double-click a sash to split the two neighbours evenly.
 */
export function Splitter({ dir, sizes, onSizes, children, className }: {
  dir: "row" | "col";
  sizes: number[];
  onSizes: (sizes: number[]) => void;
  children: React.ReactNode;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const kids = Children.toArray(children);

  const startDrag = (i: number) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const el = ref.current!;
    const rect = el.getBoundingClientRect();
    const total = dir === "row" ? rect.width : rect.height;
    const start = dir === "row" ? e.clientX : e.clientY;
    const base = [...sizes];
    const min = Math.min(MIN_PX / Math.max(total, 1), (base[i] + base[i + 1]) / 2);
    const sash = e.currentTarget as HTMLElement;
    sash.setPointerCapture(e.pointerId);
    sash.classList.add("dragging");
    document.body.classList.add(dir === "row" ? "resizing-col" : "resizing-row");
    const move = (ev: PointerEvent) => {
      const d = ((dir === "row" ? ev.clientX : ev.clientY) - start) / total;
      const pair = base[i] + base[i + 1];
      const a = Math.max(min, Math.min(pair - min, base[i] + d));
      const next = [...base];
      next[i] = a;
      next[i + 1] = pair - a;
      onSizes(next);
    };
    const up = () => {
      sash.classList.remove("dragging");
      document.body.classList.remove("resizing-col", "resizing-row");
      sash.removeEventListener("pointermove", move);
      sash.removeEventListener("pointerup", up);
      sash.removeEventListener("pointercancel", up);
    };
    sash.addEventListener("pointermove", move);
    sash.addEventListener("pointerup", up);
    sash.addEventListener("pointercancel", up);
  };

  const equalize = (i: number) => () => {
    const next = [...sizes];
    const pair = next[i] + next[i + 1];
    next[i] = next[i + 1] = pair / 2;
    onSizes(next);
  };

  return (
    <div ref={ref} className={`splitter splitter-${dir}${className ? " " + className : ""}`}>
      {kids.map((child, i) => (
        <Fragment key={i}>
          <div className="split-cell" style={{ flex: `${sizes[i] ?? 1} 1 0px` }}>{child}</div>
          {i < kids.length - 1 && (
            <div className={`sash sash-${dir}`} onPointerDown={startDrag(i)} onDoubleClick={equalize(i)} />
          )}
        </Fragment>
      ))}
    </div>
  );
}
