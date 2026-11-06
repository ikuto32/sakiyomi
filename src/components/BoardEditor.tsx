import { useState } from "react";
import { api, SessionView } from "../api";
import { runOk } from "../store";
import { HAND_KINDS, KIND_CHAR, START_SFEN, parseSfenBoard, promotable, toSfen, unpromote } from "../shogi";
import { Board, PieceGlyph } from "./Board";
import { sideName, t } from "../i18n";

type Brush = { kind: number; color: number } | "erase" | null;

const PALETTE = [1, 2, 3, 4, 7, 5, 6, 8, 9, 10, 11, 12, 13, 14];
const MAX = [0, 18, 4, 4, 4, 2, 2, 4, 2];

/** Board-setup mode: place pieces, edit hands, choose the side to move. */
export function BoardEditor({ v, flipped, onDone }: { v: SessionView; flipped: boolean; onDone: () => void }) {
  const [pos, setPos] = useState(() => parseSfenBoard(v.board.sfen));
  const [brush, setBrush] = useState<Brush>(null);
  const [picked, setPicked] = useState<number | null>(null);

  const setSquare = (q: number, p: number) => {
    const squares = [...pos.squares];
    squares[q] = p;
    setPos({ ...pos, squares });
  };

  const onEditSquare = (q: number, button: number) => {
    const p = pos.squares[q];
    if (button === 2) {
      // Right click: 先手 → 先手成 → 後手 → 後手成 → 先手
      if (!p) return;
      const kind = p & 15;
      const color = p >> 4;
      const base = unpromote(kind);
      let next: number;
      if (promotable(base) && kind === base) next = (base + 8) | (color << 4);
      else next = base | ((1 - color) << 4);
      setSquare(q, next);
      return;
    }
    if (brush === "erase") return setSquare(q, 0);
    if (brush) return setSquare(q, brush.kind | (brush.color << 4));
    if (picked === null) {
      if (p) setPicked(q);
      return;
    }
    if (picked !== q) {
      const squares = [...pos.squares];
      squares[q] = squares[picked];
      squares[picked] = 0;
      setPos({ ...pos, squares });
    }
    setPicked(null);
  };

  const onEditHand = (color: number, index: number, button: number) => {
    const hands = pos.hands.map((h) => [...h]);
    hands[color][index] = Math.max(0, Math.min(18, hands[color][index] + (button === 2 ? -1 : 1)));
    setPos({ ...pos, hands });
  };

  /** Everything not on the board or in Black's hand goes to White's hand (詰将棋 style). */
  const restToWhite = () => {
    const left = [...MAX];
    pos.squares.forEach((p) => p && (left[unpromote(p & 15)] -= 1));
    pos.hands[0].forEach((n, i) => (left[HAND_KINDS[i]] -= n));
    const hands = [pos.hands[0].slice(), HAND_KINDS.map((k) => Math.max(0, left[k]))];
    setPos({ ...pos, hands });
  };

  const apply = async () => {
    const sfen = toSfen(pos.squares, pos.hands, pos.side);
    const ok = await runOk(api.setPosition(v.id, sfen));
    if (ok) onDone();
  };

  return (
    <div className="editor">
      <div className="editor-board">
        <Board
          squares={pos.squares}
          hands={pos.hands}
          side={pos.side}
          flipped={flipped}
          names={[sideName(0), sideName(1)]}
          onEditSquare={onEditSquare}
          onEditHand={onEditHand}
          editSelected={picked}
          showEmptyHands
        />
      </div>
      <div className="editor-panel">
        <h3>{t("局面編集")}</h3>
        <p className="muted small">
          {t("駒を選んで盤をクリックで配置／駒をクリック→移動先クリックで移動／右クリックで成・先後切替／駒台は左クリックで＋1、右クリックで−1")}
        </p>
        {[0, 1].map((color) => (
          <div key={color} className="palette">
            <span className="palette-label">{color ? "☖" : "☗"}</span>
            {PALETTE.map((k) => {
              const on = brush !== null && brush !== "erase" && brush.kind === k && brush.color === color;
              return (
                <button key={k} className={"palette-btn" + (on ? " on" : "")} title={KIND_CHAR[k]} onClick={() => setBrush(on ? null : { kind: k, color })}>
                  <svg viewBox="0 0 60 60"><PieceGlyph kind={k} color={color} x={0} y={0} down={color === 1} /></svg>
                </button>
              );
            })}
          </div>
        ))}
        <div className="editor-tools">
          <button className={brush === "erase" ? "on" : ""} onClick={() => setBrush(brush === "erase" ? null : "erase")}>{t("消しゴム")}</button>
          <button className={brush === null ? "on" : ""} onClick={() => setBrush(null)}>{t("移動")}</button>
        </div>
        <div className="editor-tools">
          <button onClick={() => setPos(parseSfenBoard(START_SFEN))}>{t("平手初期配置")}</button>
          <button onClick={() => setPos({ squares: new Array(81).fill(0), hands: [new Array(7).fill(0), new Array(7).fill(0)], side: 0 })}>{t("盤面クリア")}</button>
          <button onClick={restToWhite}>{t("残り駒を後手の駒台へ")}</button>
        </div>
        <div className="radio-row">
          <span>{t("手番:")}</span>
          <label><input type="radio" checked={pos.side === 0} onChange={() => setPos({ ...pos, side: 0 })} /> {t("先手")}</label>
          <label><input type="radio" checked={pos.side === 1} onChange={() => setPos({ ...pos, side: 1 })} /> {t("後手")}</label>
        </div>
        <code className="sfen-preview">{toSfen(pos.squares, pos.hands, pos.side)}</code>
        <div className="dialog-actions">
          <span className="spacer" />
          <button onClick={onDone}>{t("キャンセル")}</button>
          <button className="primary" onClick={apply}>{t("この局面で開始")}</button>
        </div>
      </div>
    </div>
  );
}
