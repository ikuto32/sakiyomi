import { useMemo, useRef, useState } from "react";
import { usePieceLabels } from "../settings";
import { rankLabels } from "../i18n";
import {
  HAND_DISPLAY, HAND_KINDS, KIND_CHAR, LegalIndex, fileOf, promotable, rankOf, sq, usiMove,
} from "../shogi";

const S = 60; // square size in SVG units
const BX = 118; // board origin
const BY = 34;
const W = BX + 9 * S + 150;
const H = BY + 9 * S + 26;
const HAND_SLOT = 58;
const HAND_PIECE = S * 0.92; // centred in its slot so the two stands mirror exactly

export interface BoardProps {
  squares: number[];
  hands: number[][];
  side: number;
  flipped: boolean;
  names: [string, string];
  /** Interactive when present. */
  legal?: LegalIndex | null;
  onMove?: (usi: string) => void;
  lastMove?: { from: number | null; to: number | null } | null;
  inCheck?: boolean;
  /** Best-move arrow (USI) from analysis. */
  arrow?: string | null;
  /** Board-editing hooks (replace normal move input). */
  onEditSquare?: (sq: number, button: number) => void;
  onEditHand?: (color: number, handIndex: number, button: number) => void;
  editSelected?: number | null;
  showEmptyHands?: boolean;
  /** Which side the clock says is to move (for name highlighting). */
  activeSide?: number | null;
}

type Sel = { from: number } | { drop: number } | null;

function squareXY(q: number, flipped: boolean) {
  const col = flipped ? fileOf(q) - 1 : 9 - fileOf(q);
  const row = flipped ? 9 - rankOf(q) : rankOf(q) - 1;
  return { x: BX + col * S, y: BY + row * S };
}

function squareAt(x: number, y: number, flipped: boolean): number | null {
  const col = Math.floor((x - BX) / S);
  const row = Math.floor((y - BY) / S);
  if (col < 0 || col > 8 || row < 0 || row > 8) return null;
  const file = flipped ? col + 1 : 9 - col;
  const rank = flipped ? 9 - row : row + 1;
  return sq(file, rank);
}

const HAND_X_BOTTOM = BX + 9 * S + 46;
/** Mirror of the bottom stand through the board centre. */
const HAND_X_TOP = 2 * (BX + 4.5 * S) - HAND_X_BOTTOM - S;

/**
 * Screen layout of a hand. The two stands are point-symmetric, like turning the board
 * around: the bottom stand stacks 歩 (nearest its owner) up to 飛, and the top stand
 * stacks 歩 (nearest its owner, at the top) down to 飛.
 */
export function handSlots(color: number, flipped: boolean, hand: number[], showEmpty: boolean) {
  const top = color === (flipped ? 0 : 1);
  const kinds = HAND_DISPLAY.filter((i) => showEmpty || hand[i] > 0);
  const order = top ? [...kinds].reverse() : kinds;
  return order.map((i, n) => ({
    index: i,
    kind: HAND_KINDS[i],
    count: hand[i],
    top,
    x: top ? HAND_X_TOP : HAND_X_BOTTOM,
    // Exact mirror images of each other through the board centre.
    y: top ? BY + 30 + n * HAND_SLOT : BY + 9 * S - 30 - (order.length - n) * HAND_SLOT,
  }));
}

const TWO_CHAR: Record<number, string> = {
  1: "歩兵", 2: "香車", 3: "桂馬", 4: "銀将", 5: "角行", 6: "飛車", 7: "金将",
  9: "と金", 10: "成香", 11: "成桂", 12: "成銀", 13: "龍馬", 14: "龍王",
};

export function PieceGlyph({ kind, x, y, down, size = S, ghost = false, color }: {
  kind: number; x: number; y: number; down: boolean; size?: number; ghost?: boolean; color: number;
}) {
  const labels = usePieceLabels();
  const k = kind & 15;
  const ch = k === 8 ? (color === 0 ? "王" : "玉") : KIND_CHAR[k];
  const two = k === 8 ? (color === 0 ? "王将" : "玉将") : TWO_CHAR[k];
  const s = size;
  const cx = x + s / 2;
  const cy = y + s / 2;
  const pts = [
    [0.5, 0.07], [0.8, 0.19], [0.89, 0.93], [0.11, 0.93], [0.2, 0.19],
  ].map(([px, py]) => `${x + px * s},${y + py * s}`).join(" ");
  return (
    <g transform={down ? `rotate(180 ${cx} ${cy})` : undefined} opacity={ghost ? 0.85 : 1} pointerEvents="none">
      <polygon points={pts} className="piece-body" />
      {labels === "two" ? (
        <text className={k > 8 ? "piece-text promoted" : "piece-text"} fontSize={s * 0.33} textAnchor="middle" dominantBaseline="middle">
          <tspan x={cx} y={y + s * 0.44}>{two[0]}</tspan>
          <tspan x={cx} y={y + s * 0.76}>{two[1]}</tspan>
        </text>
      ) : (
        <text x={cx} y={y + s * 0.62} className={k > 8 ? "piece-text promoted" : "piece-text"} fontSize={s * 0.5} textAnchor="middle" dominantBaseline="middle">
          {ch}
        </text>
      )}
    </g>
  );
}

export function Board(props: BoardProps) {
  const { squares, hands, side, flipped, legal, onMove, names } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const [sel, setSel] = useState<Sel>(null);
  const [drag, setDrag] = useState<{ x: number; y: number; sx: number; sy: number; active: boolean; piece: number } | null>(null);
  const [promo, setPromo] = useState<{ from: number; to: number } | null>(null);
  const editing = !!props.onEditSquare;
  const showEmpty = !!props.showEmptyHands;

  // Selection is only meaningful for the position it was made in.
  const posKey = squares.join(",") + "|" + side + "|" + (legal ? "1" : "0");
  const lastPos = useRef(posKey);
  if (lastPos.current !== posKey) {
    lastPos.current = posKey;
    if (sel) setSel(null);
    if (promo) setPromo(null);
    if (drag) setDrag(null);
  }

  const targets = useMemo(() => {
    const t = new Set<number>();
    if (!legal || !sel) return t;
    if ("from" in sel) legal.board.get(sel.from)?.forEach((_, to) => t.add(to));
    else legal.drops.get(sel.drop)?.forEach((to) => t.add(to));
    return t;
  }, [legal, sel]);

  const toSvg = (e: React.PointerEvent | React.MouseEvent) => {
    const svg = svgRef.current!;
    const pt = svg.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const m = svg.getScreenCTM();
    const p = m ? pt.matrixTransform(m.inverse()) : pt;
    return { x: p.x, y: p.y };
  };

  const handAt = (x: number, y: number) => {
    for (const color of [0, 1]) {
      for (const slot of handSlots(color, flipped, hands[color], showEmpty)) {
        if (x >= slot.x && x <= slot.x + S && y >= slot.y && y <= slot.y + HAND_SLOT) return { color, ...slot };
      }
    }
    return null;
  };

  const tryMove = (s: Sel, to: number) => {
    if (!legal || !s || !onMove) return false;
    if ("drop" in s) {
      if (!legal.drops.get(s.drop)?.has(to)) return false;
      onMove(usiMove(null, to, false, s.drop));
      setSel(null);
      return true;
    }
    const e = legal.board.get(s.from)?.get(to);
    if (!e) return false;
    if (e.plain && e.promote) {
      setPromo({ from: s.from, to });
    } else {
      onMove(usiMove(s.from, to, e.promote));
      setSel(null);
    }
    return true;
  };

  const onPointerDown = (e: React.PointerEvent) => {
    const { x, y } = toSvg(e);
    if (promo) return;
    if (editing) {
      const q = squareAt(x, y, flipped);
      if (q !== null) props.onEditSquare!(q, e.button);
      else {
        const h = handAt(x, y);
        if (h) props.onEditHand?.(h.color, h.index, e.button);
      }
      return;
    }
    if (!legal || e.button !== 0) return;
    const q = squareAt(x, y, flipped);
    if (q !== null) {
      if (sel && targets.has(q)) {
        tryMove(sel, q);
        return;
      }
      const p = squares[q];
      if (p && p >> 4 === side && legal.board.has(q)) {
        setSel({ from: q });
        setDrag({ x, y, sx: x, sy: y, active: false, piece: p });
        svgRef.current?.setPointerCapture(e.pointerId);
        return;
      }
      setSel(null);
      return;
    }
    const h = handAt(x, y);
    if (h && h.color === side && legal.drops.has(h.kind)) {
      setSel({ drop: h.kind });
      setDrag({ x, y, sx: x, sy: y, active: false, piece: h.kind | (side << 4) });
      svgRef.current?.setPointerCapture(e.pointerId);
      return;
    }
    setSel(null);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const { x, y } = toSvg(e);
    const active = drag.active || Math.hypot(x - drag.sx, y - drag.sy) > 6;
    setDrag({ ...drag, x, y, active });
  };

  const onPointerUp = (e: React.PointerEvent) => {
    if (!drag) return;
    const wasActive = drag.active;
    setDrag(null);
    if (!wasActive) return;
    const { x, y } = toSvg(e);
    const q = squareAt(x, y, flipped);
    if (q !== null && targets.has(q)) tryMove(sel, q);
  };

  const last = props.lastMove;
  const kingSq = props.inCheck ? squares.findIndex((p) => p === (8 | (side << 4))) : -1;
  const dragFrom = drag?.active && sel && "from" in sel ? sel.from : null;

  const arrow = useMemo(() => {
    const a = props.arrow;
    if (!a || a.length < 4) return null;
    const toSq = sq(Number(a[2]), a.charCodeAt(3) - 96);
    const t = squareXY(toSq, flipped);
    let f: { x: number; y: number };
    if (a[1] === "*") {
      const kind = "PLNSBRG".indexOf(a[0]) + 1;
      const slot = handSlots(side, flipped, hands[side], showEmpty).find((s) => s.kind === kind);
      if (!slot) return null;
      f = { x: slot.x, y: slot.y };
    } else {
      f = squareXY(sq(Number(a[0]), a.charCodeAt(1) - 96), flipped);
    }
    return { x1: f.x + S / 2, y1: f.y + S / 2, x2: t.x + S / 2, y2: t.y + S / 2 };
  }, [props.arrow, flipped, side, hands, showEmpty]);

  const nameFor = (color: number) => {
    const top = color === (flipped ? 0 : 1);
    const mark = color === 0 ? "☗" : "☖";
    const active = props.activeSide === color;
    return (
      <text
        x={top ? HAND_X_TOP - 4 : BX + 9 * S + 42}
        y={top ? BY + 12 : BY + 9 * S - 4}
        className={active ? "hand-name active" : "hand-name"}
      >
        {mark} {names[color].length > 9 ? names[color].slice(0, 9) + "…" : names[color]}
      </text>
    );
  };

  return (
    <svg
      ref={svgRef}
      className={"board" + (editing ? " editing" : "")}
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="xMidYMid meet"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onContextMenu={(e) => e.preventDefault()}
    >
      <defs>
        <linearGradient id="wood" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" className="wood-a" />
          <stop offset="1" className="wood-b" />
        </linearGradient>
        <marker id="arrowhead" markerWidth="4" markerHeight="4" refX="2" refY="2" orient="auto">
          <path d="M0,0 L4,2 L0,4 z" className="arrow-head" />
        </marker>
      </defs>
      <rect x={BX - 8} y={BY - 8} width={9 * S + 16} height={9 * S + 16} rx={10} className="board-frame" />
      <rect x={BX} y={BY} width={9 * S} height={9 * S} fill="url(#wood)" />
      {Array.from({ length: 10 }, (_, i) => (
        <g key={i} className="grid">
          <line x1={BX + i * S} y1={BY} x2={BX + i * S} y2={BY + 9 * S} />
          <line x1={BX} y1={BY + i * S} x2={BX + 9 * S} y2={BY + i * S} />
        </g>
      ))}
      {[3, 6].flatMap((a) => [3, 6].map((b) => (
        <circle key={`${a}${b}`} cx={BX + a * S} cy={BY + b * S} r={3.2} className="star" />
      )))}
      {Array.from({ length: 9 }, (_, i) => (
        <g key={i} className="coord">
          <text x={BX + i * S + S / 2} y={BY - 14} textAnchor="middle">{flipped ? i + 1 : 9 - i}</text>
          <text x={BX + 9 * S + 18} y={BY + i * S + S / 2 + 5} textAnchor="middle">{rankLabels()[flipped ? 8 - i : i]}</text>
        </g>
      ))}
      {last?.from != null && (() => { const p = squareXY(last.from!, flipped); return <rect x={p.x} y={p.y} width={S} height={S} className="hl-last-from" />; })()}
      {last?.to != null && (() => { const p = squareXY(last.to!, flipped); return <rect x={p.x} y={p.y} width={S} height={S} className="hl-last" />; })()}
      {kingSq >= 0 && (() => { const p = squareXY(kingSq, flipped); return <rect x={p.x} y={p.y} width={S} height={S} className="hl-check" />; })()}
      {sel && "from" in sel && (() => { const p = squareXY(sel.from, flipped); return <rect x={p.x} y={p.y} width={S} height={S} className="hl-selected" />; })()}
      {props.editSelected != null && (() => { const p = squareXY(props.editSelected!, flipped); return <rect x={p.x} y={p.y} width={S} height={S} className="hl-selected" />; })()}
      {squares.map((p, q) => {
        if (!p || q === dragFrom) return null;
        const { x, y } = squareXY(q, flipped);
        const color = p >> 4;
        return <PieceGlyph key={q} kind={p & 15} color={color} x={x} y={y} down={color === (flipped ? 0 : 1)} />;
      })}
      {[...targets].map((q) => {
        const { x, y } = squareXY(q, flipped);
        return squares[q]
          ? <rect key={q} x={x + 2} y={y + 2} width={S - 4} height={S - 4} rx={6} className="hl-capture" />
          : <circle key={q} cx={x + S / 2} cy={y + S / 2} r={8} className="hl-target" />;
      })}
      {[0, 1].map((color) => (
        <g key={color}>
          {nameFor(color)}
          {handSlots(color, flipped, hands[color], showEmpty).map((slot) => {
            const selected = sel && "drop" in sel && sel.drop === slot.kind && color === side;
            return (
              <g key={slot.index} className={slot.count ? "" : "hand-empty"}>
                {selected && <rect x={slot.x - 2} y={slot.y - 2} width={S + 4} height={HAND_SLOT} rx={8} className="hl-selected" />}
                <PieceGlyph kind={slot.kind} color={color} x={slot.x + (S - HAND_PIECE) / 2} y={slot.y + (HAND_SLOT - HAND_PIECE) / 2} size={HAND_PIECE} down={color === (flipped ? 0 : 1)} />
                {(slot.count > 1 || showEmpty) && (
                  <g pointerEvents="none">
                    {/* Badge mirrored with the stand: top-right below, bottom-left above. */}
                    <circle cx={slot.top ? slot.x + 4 : slot.x + S - 4} cy={slot.top ? slot.y + HAND_SLOT - 12 : slot.y + 12} r={11} className="hand-badge" />
                    <text x={slot.top ? slot.x + 4 : slot.x + S - 4} y={slot.top ? slot.y + HAND_SLOT - 7.5 : slot.y + 16.5} textAnchor="middle" className="hand-count">{slot.count}</text>
                  </g>
                )}
              </g>
            );
          })}
        </g>
      ))}
      {arrow && (
        <line {...arrow} className="best-arrow" markerEnd="url(#arrowhead)" pointerEvents="none" />
      )}
      {drag?.active && (
        <PieceGlyph kind={drag.piece & 15} color={drag.piece >> 4} x={drag.x - S / 2} y={drag.y - S / 2} down={(drag.piece >> 4) === (flipped ? 0 : 1)} ghost />
      )}
      {promo && (() => {
        const p = squareXY(promo.to, flipped);
        const kind = squares[promo.from] & 15;
        const x = Math.min(p.x, BX + 8 * S - S * 0.2);
        const y = Math.min(Math.max(p.y - S / 2, BY), BY + 7 * S);
        const choose = (promote: boolean) => (ev: React.PointerEvent) => {
          ev.stopPropagation();
          onMove?.(usiMove(promo.from, promo.to, promote));
          setPromo(null);
          setSel(null);
        };
        return (
          <g className="promo">
            <rect x={BX} y={BY} width={9 * S} height={9 * S} className="promo-shade" onPointerDown={(ev) => { ev.stopPropagation(); setPromo(null); }} />
            <rect x={x - 6} y={y - 6} width={S * 1.2 + 12} height={S * 2 + 18} rx={10} className="promo-box" />
            <g onPointerDown={choose(true)} className="promo-choice">
              <rect x={x} y={y} width={S * 1.2} height={S} rx={8} />
              <PieceGlyph kind={promotable(kind) ? kind + 8 : kind} color={side} x={x + S * 0.1} y={y} down={false} />
            </g>
            <g onPointerDown={choose(false)} className="promo-choice">
              <rect x={x} y={y + S + 6} width={S * 1.2} height={S} rx={8} />
              <PieceGlyph kind={kind} color={side} x={x + S * 0.1} y={y + S + 6} down={false} />
            </g>
          </g>
        );
      })()}
    </svg>
  );
}
