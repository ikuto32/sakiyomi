import { Fragment, useEffect, useRef, useState } from "react";
import {
  Group, LayoutNode, ViewId, Zone, activate, groups, placeView, removeView, setSizes, viewTitle, views,
} from "../layout";
import { Splitter } from "./Splitter";
import { t as tr } from "../i18n";

interface DragState {
  view: ViewId;
  from: string;
  sx: number;
  sy: number;
  x: number;
  y: number;
  active: boolean;
  target: { group: string; zone: Zone; index?: number } | null;
}

interface MenuState {
  x: number;
  y: number;
  group: string;
  view: ViewId;
}

/**
 * Tabbed, splittable panel area. Drag a tab onto another group's edge to split,
 * onto its centre or tab strip to stack; drag sashes to resize; double-click a tab to maximize.
 */
export function Dock({ layout, onChange, renderView }: {
  layout: LayoutNode;
  onChange: (l: LayoutNode) => void;
  renderView: (v: ViewId) => React.ReactNode;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [maximized, setMaximized] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const dragRef = useRef<DragState | null>(null);
  dragRef.current = drag;

  const maxGroup = maximized ? groups(layout).find((g) => g.id === maximized) : undefined;
  const single = views(layout).length <= 1;

  // Pointer-driven drag (HTML5 drag-and-drop is unavailable while Tauri handles file drops).
  useEffect(() => {
    if (!drag) return;
    const hit = (x: number, y: number): DragState["target"] => {
      const el = document.elementFromPoint(x, y) as HTMLElement | null;
      if (!el || !rootRef.current?.contains(el)) return null;
      const strip = el.closest("[data-tabs]") as HTMLElement | null;
      if (strip) {
        const tabs = [...strip.querySelectorAll<HTMLElement>("[data-tab]")];
        let index = tabs.length;
        tabs.forEach((t, i) => {
          const r = t.getBoundingClientRect();
          if (index === tabs.length && x < r.left + r.width / 2) index = i;
        });
        return { group: strip.dataset.tabs!, zone: "center", index };
      }
      const content = el.closest("[data-content]") as HTMLElement | null;
      if (!content) return null;
      const r = content.getBoundingClientRect();
      const rx = (x - r.left) / r.width;
      const ry = (y - r.top) / r.height;
      const edges: [Zone, number][] = [["left", rx], ["right", 1 - rx], ["top", ry], ["bottom", 1 - ry]];
      edges.sort((a, b) => a[1] - b[1]);
      return { group: content.dataset.content!, zone: edges[0][1] < 0.25 ? edges[0][0] : "center" };
    };
    const move = (e: PointerEvent) => {
      setDrag((d) => {
        if (!d) return d;
        const active = d.active || Math.hypot(e.clientX - d.sx, e.clientY - d.sy) > 5;
        return { ...d, x: e.clientX, y: e.clientY, active, target: active ? hit(e.clientX, e.clientY) : null };
      });
    };
    const up = (e: PointerEvent) => {
      const d = dragRef.current;
      setDrag(null);
      if (!d) return;
      // Moves may be coalesced, so decide from the release point itself.
      const active = d.active || Math.hypot(e.clientX - d.sx, e.clientY - d.sy) > 5;
      const target = active ? hit(e.clientX, e.clientY) : null;
      if (active && target) {
        const t = target;
        onChange(placeView(layoutRef.current, d.view, t.group, t.zone, t.index));
      } else if (!active) {
        onChange(activate(layoutRef.current, d.from, d.view));
      }
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setDrag(null);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("keydown", key);
    };
    // Only (re)subscribe when a drag starts or ends.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drag !== null]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menu]);

  const close = (view: ViewId) => {
    if (single) return;
    const next = removeView(layout, view);
    if (next) onChange(next);
  };
  const moveTo = (g: string, view: ViewId, zone: Zone) => onChange(placeView(layout, view, g, zone));

  const renderGroup = (g: Group) => {
    const target = drag?.active && drag.target?.group === g.id ? drag.target : null;
    return (
      <div className={"dock-group" + (maximized === g.id ? " maximized" : "")} key={g.id}>
        <div className="dock-tabs" data-tabs={g.id} onDoubleClick={(e) => e.target === e.currentTarget && setMaximized(maximized ? null : g.id)}>
          {g.tabs.map((t, i) => (
            <Fragment key={t}>
            {target?.zone === "center" && target.index === i && <span className="tab-drop-marker" />}
            <div
              data-tab={t}
              className={"dock-tab" + (t === g.active ? " active" : "") + (drag?.active && drag.view === t ? " dragging" : "")}
              onPointerDown={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  close(t);
                  return;
                }
                if (e.button !== 0) return;
                // Capture so moves keep arriving while the button is held (WebView2).
                (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                setDrag({ view: t, from: g.id, sx: e.clientX, sy: e.clientY, x: e.clientX, y: e.clientY, active: false, target: null });
              }}
              onDoubleClick={() => setMaximized(maximized ? null : g.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, group: g.id, view: t });
              }}
              title={tr("ドラッグで移動・分割／ダブルクリックで最大化／中クリックで閉じる")}
            >
              <span>{viewTitle(t)}</span>
              {!single && (
                <button
                  className="tab-close"
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={() => close(t)}
                  aria-label={tr("{name}を閉じる", { name: viewTitle(t) })}
                >
                  ×
                </button>
              )}
            </div>
            </Fragment>
          ))}
          {target?.zone === "center" && target.index === g.tabs.length && <span className="tab-drop-marker" />}
          <span className="dock-tabs-fill" />
          <button className="dock-action" title={tr(maximized === g.id ? "元に戻す" : "最大化")} onClick={() => setMaximized(maximized === g.id ? null : g.id)}>
            {maximized === g.id ? "🗗" : "🗖"}
          </button>
        </div>
        <div className="dock-content" data-content={g.id}>
          {renderView(g.active)}
          {target && target.index === undefined && <div className={`drop-zone drop-${target.zone}`} />}
        </div>
      </div>
    );
  };

  const renderNode = (n: LayoutNode): React.ReactNode =>
    n.type === "group" ? renderGroup(n) : (
      <Splitter key={n.id} dir={n.dir} sizes={n.sizes} onSizes={(s) => onChange(setSizes(layoutRef.current, n.id, s))}>
        {n.children.map(renderNode)}
      </Splitter>
    );

  return (
    <div className={"dock" + (drag?.active ? " drag-active" : "")} ref={rootRef}>
      {maxGroup ? renderGroup(maxGroup) : renderNode(layout)}
      {drag?.active && (
        <div className="drag-ghost" style={{ left: drag.x + 12, top: drag.y + 10 }}>{viewTitle(drag.view)}</div>
      )}
      {menu && (
        <div className="context-menu" style={{ left: menu.x, top: menu.y }} onPointerDown={(e) => e.stopPropagation()}>
          {([
            [tr("右に分割"), "right"],
            [tr("下に分割"), "bottom"],
            [tr("左に分割"), "left"],
            [tr("上に分割"), "top"],
          ] as [string, Zone][]).map(([label, zone]) => {
            const g = groups(layout).find((x) => x.id === menu.group);
            return (
              <button key={zone} disabled={!g || g.tabs.length < 2} onClick={() => { moveTo(menu.group, menu.view, zone); setMenu(null); }}>
                {label}
              </button>
            );
          })}
          <hr />
          <button onClick={() => { setMaximized(maximized === menu.group ? null : menu.group); setMenu(null); }}>
            {tr(maximized === menu.group ? "最大化を解除" : "最大化")}
          </button>
          <button disabled={single} onClick={() => { close(menu.view); setMenu(null); }}>{tr("閉じる")}</button>
        </div>
      )}
    </div>
  );
}
