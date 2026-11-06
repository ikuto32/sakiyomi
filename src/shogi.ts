// Board helpers mirroring shogi-core encodings.
// Square index = (file - 1) * 9 + (rank - 1); piece = kind | color << 4.
// Kinds: 1 歩 2 香 3 桂 4 銀 5 角 6 飛 7 金 8 玉, 9..14 promoted (と 杏 圭 全 馬 龍).

import type { Eval } from "./api";
import { t } from "./i18n";

export const HAND_KINDS = [1, 2, 3, 4, 7, 5, 6]; // shogi-core hand index order
export const HAND_DISPLAY = [6, 5, 4, 3, 2, 1, 0]; // 飛 角 金 銀 桂 香 歩
const USI_LETTERS = "PLNSBRGK";
const SFEN_LETTERS = "PLNSBRGK";

export const KIND_CHAR: Record<number, string> = {
  1: "歩", 2: "香", 3: "桂", 4: "銀", 5: "角", 6: "飛", 7: "金", 8: "玉",
  9: "と", 10: "杏", 11: "圭", 12: "全", 13: "馬", 14: "龍",
};

export const fileOf = (sq: number) => Math.floor(sq / 9) + 1;
export const rankOf = (sq: number) => (sq % 9) + 1;
export const sq = (file: number, rank: number) => (file - 1) * 9 + rank - 1;
export const sqFromUsi = (s: string) => sq(Number(s[0]), s.charCodeAt(1) - 96);
export const sqToUsi = (q: number) => `${fileOf(q)}${String.fromCharCode(96 + rankOf(q))}`;
export const promotable = (kind: number) => kind >= 1 && kind <= 6;
export const unpromote = (kind: number) => (kind > 8 ? kind - 8 : kind);

export interface LegalIndex {
  /** from square → destination → {plain, promote} availability */
  board: Map<number, Map<number, { plain: boolean; promote: boolean }>>;
  /** drop kind → destination squares */
  drops: Map<number, Set<number>>;
}

export function indexLegal(legal: string[]): LegalIndex {
  const board = new Map<number, Map<number, { plain: boolean; promote: boolean }>>();
  const drops = new Map<number, Set<number>>();
  for (const m of legal) {
    if (m[1] === "*") {
      const kind = USI_LETTERS.indexOf(m[0]) + 1;
      if (!drops.has(kind)) drops.set(kind, new Set());
      drops.get(kind)!.add(sqFromUsi(m.slice(2, 4)));
      continue;
    }
    const from = sqFromUsi(m.slice(0, 2));
    const to = sqFromUsi(m.slice(2, 4));
    if (!board.has(from)) board.set(from, new Map());
    const dests = board.get(from)!;
    const e = dests.get(to) ?? { plain: false, promote: false };
    if (m.endsWith("+")) e.promote = true;
    else e.plain = true;
    dests.set(to, e);
  }
  return { board, drops };
}

export function usiMove(from: number | null, to: number, promote: boolean, dropKind = 0) {
  if (from === null) return `${USI_LETTERS[dropKind - 1]}*${sqToUsi(to)}`;
  return `${sqToUsi(from)}${sqToUsi(to)}${promote ? "+" : ""}`;
}

/** Build an SFEN from editor state (squares/hands as in BoardView). */
export function toSfen(squares: number[], hands: number[][], side: number, moveNumber = 1): string {
  const rows: string[] = [];
  for (let rank = 1; rank <= 9; rank++) {
    let row = "";
    let empty = 0;
    for (let file = 9; file >= 1; file--) {
      const p = squares[sq(file, rank)];
      if (!p) {
        empty++;
        continue;
      }
      if (empty) row += empty;
      empty = 0;
      let kind = p & 15;
      if (kind > 8) {
        row += "+";
        kind -= 8;
      }
      const c = SFEN_LETTERS[kind - 1];
      row += p >> 4 ? c.toLowerCase() : c;
    }
    if (empty) row += empty;
    rows.push(row);
  }
  let hand = "";
  for (const color of [0, 1]) {
    for (const i of HAND_DISPLAY) {
      const n = hands[color][i];
      if (!n) continue;
      const c = SFEN_LETTERS[HAND_KINDS[i] - 1];
      hand += (n > 1 ? n : "") + (color ? c.toLowerCase() : c);
    }
  }
  return `${rows.join("/")} ${side ? "w" : "b"} ${hand || "-"} ${moveNumber}`;
}

export const START_SFEN = "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1";

export function parseSfenBoard(sfen: string): { squares: number[]; hands: number[][]; side: number } {
  const [board, side, hand] = sfen.split(" ");
  const squares = new Array(81).fill(0);
  board.split("/").forEach((row, r) => {
    let file = 9;
    let promoted = false;
    for (const ch of row) {
      if (ch === "+") {
        promoted = true;
        continue;
      }
      if (/[1-9]/.test(ch)) {
        file -= Number(ch);
        continue;
      }
      const kind = SFEN_LETTERS.indexOf(ch.toUpperCase()) + 1 + (promoted ? 8 : 0);
      squares[sq(file, r + 1)] = kind | (ch === ch.toLowerCase() ? 16 : 0);
      file--;
      promoted = false;
    }
  });
  const hands = [new Array(7).fill(0), new Array(7).fill(0)];
  if (hand && hand !== "-") {
    let n = 0;
    for (const ch of hand) {
      if (/[0-9]/.test(ch)) {
        n = n * 10 + Number(ch);
        continue;
      }
      const kind = SFEN_LETTERS.indexOf(ch.toUpperCase()) + 1;
      const color = ch === ch.toLowerCase() ? 1 : 0;
      hands[color][HAND_KINDS.indexOf(kind)] += n || 1;
      n = 0;
    }
  }
  return { squares, hands, side: side === "w" ? 1 : 0 };
}

export function formatEval(e: Eval | null | undefined): string {
  if (!e) return "—";
  if (e.mate !== null) {
    const sign = e.mate >= 0 ? "+" : "-";
    const n = Math.abs(e.mate);
    return n >= 999 ? `${sign}${t("詰")}` : `${sign}${t("詰")}${n}`;
  }
  return (e.cp > 0 ? "+" : "") + e.cp;
}

export function formatClock(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(h ? 2 : 1, "0");
  return `${h ? h + ":" : ""}${mm}:${String(s).padStart(2, "0")}`;
}

export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (t("万") === "K") {
    // English: K / M / G
    if (n >= 1e9) return (n / 1e9).toFixed(1) + "G";
    if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
    if (n >= 1e4) return (n / 1e3).toFixed(0) + "K";
    return String(n);
  }
  if (n >= 1e8) return (n / 1e8).toFixed(1) + "億";
  if (n >= 1e4) return (n / 1e4).toFixed(n >= 1e6 ? 0 : 1) + "万";
  return String(n);
}

/** Line through `cursor`: root → cursor, then first children to the end. */
export function currentLine(nodes: { parent: number | null; children: number[] }[], cursor: number): number[] {
  const back: number[] = [];
  for (let n: number | null = cursor; n !== null; n = nodes[n].parent) back.push(n);
  back.reverse();
  let n = cursor;
  while (nodes[n].children.length) {
    n = nodes[n].children[0];
    back.push(n);
  }
  return back;
}
