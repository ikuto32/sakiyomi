// Appearance settings (theme, colours, fonts, zoom), persisted in localStorage.
import { createContext, useContext } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { Lang, Notation, setLang, setNotation } from "./i18n";
import type { EvalStyle } from "./api";

export type ThemeMode = "system" | "dark" | "light";
export type PieceLabels = "one" | "two";
export type LanguageSetting = "system" | Lang;
export type NotationSetting = "auto" | Notation;

export interface Settings {
  language: LanguageSetting;
  notation: NotationSetting;
  theme: ThemeMode;
  accent: string;
  board: string;
  pieceFont: string;
  pieceLabels: PieceLabels;
  /** UI text size multiplier (1 = 13px base). */
  fontScale: number;
  /** Whole-window zoom (WebView zoom, like Ctrl+= in VS Code). */
  zoom: number;
  /** How engine evaluations and PVs are written into saved records. */
  evalStyle: EvalStyle;
}

export const ACCENTS: Record<string, { name: string; dark: string; light: string }> = {
  blue: { name: "ブルー", dark: "#4f8cff", light: "#2563eb" },
  teal: { name: "ティール", dark: "#2fbfae", light: "#0f8f80" },
  purple: { name: "パープル", dark: "#a07cff", light: "#7c4dff" },
  orange: { name: "オレンジ", dark: "#ff9a3c", light: "#d9700b" },
  red: { name: "レッド", dark: "#ff6b6b", light: "#d93838" },
  green: { name: "グリーン", dark: "#56c271", light: "#23913f" },
};

export const BOARDS: Record<string, { name: string; a: string; b: string; grid: string; frame: string }> = {
  wood: { name: "木目", a: "#e9c98f", b: "#d6a85c", grid: "#6b4a1f", frame: "#8a6232" },
  light: { name: "明るい木目", a: "#f6e3b8", b: "#ebcb8b", grid: "#7a5a2c", frame: "#b08850" },
  dark: { name: "濃い木目", a: "#d9a35e", b: "#b47b37", grid: "#4a3112", frame: "#6b4520" },
  gray: { name: "グレー", a: "#e3e6ea", b: "#c9ced6", grid: "#4a5260", frame: "#7b8494" },
  green: { name: "グリーン", a: "#d6e8c8", b: "#b4d2a0", grid: "#3e5a2c", frame: "#5f7d4a" },
};

export const PIECE_FONTS: Record<string, { name: string; css: string }> = {
  mincho: { name: "明朝", css: '"Yu Mincho", "YuMincho", "Hiragino Mincho ProN", "MS Mincho", serif' },
  gothic: { name: "ゴシック", css: '"Yu Gothic UI", "Yu Gothic", "Meiryo", sans-serif' },
  kyokasho: { name: "教科書体", css: '"UD デジタル 教科書体 N-B", "UD Digi Kyokasho N-B", "UD デジタル 教科書体 N-R", "Yu Mincho", serif' },
};

export const DEFAULT_SETTINGS: Settings = {
  language: "system",
  notation: "auto",
  theme: "system",
  accent: "blue",
  board: "wood",
  pieceFont: "mincho",
  pieceLabels: "one",
  fontScale: 1,
  zoom: 1,
  evalStyle: "shogigui",
};

export const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
export const FONT_STEPS = [0.85, 0.92, 1, 1.08, 1.15, 1.25, 1.4, 1.6];

export function loadSettings(): Settings {
  try {
    const raw = JSON.parse(localStorage.getItem("settings") ?? "{}");
    const s = { ...DEFAULT_SETTINGS, ...raw } as Settings;
    if (!["system", "dark", "light"].includes(s.theme)) s.theme = "system";
    if (!["system", "ja", "en"].includes(s.language)) s.language = "system";
    if (!["auto", "ja", "western", "usi"].includes(s.notation)) s.notation = "auto";
    if (!ACCENTS[s.accent]) s.accent = DEFAULT_SETTINGS.accent;
    if (!BOARDS[s.board]) s.board = DEFAULT_SETTINGS.board;
    if (!PIECE_FONTS[s.pieceFont]) s.pieceFont = DEFAULT_SETTINGS.pieceFont;
    if (s.pieceLabels !== "two") s.pieceLabels = "one";
    if (!["shogigui", "shogihome", "floodgate", "none"].includes(s.evalStyle)) s.evalStyle = DEFAULT_SETTINGS.evalStyle;
    s.fontScale = clamp(Number(s.fontScale) || 1, 0.75, 1.8);
    s.zoom = clamp(Number(s.zoom) || 1, 0.5, 2);
    return s;
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

export function saveSettings(s: Settings) {
  try {
    localStorage.setItem("settings", JSON.stringify(s));
  } catch {
    /* storage unavailable */
  }
}

export const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

export function step(values: number[], current: number, dir: 1 | -1): number {
  if (dir > 0) return values.find((v) => v > current + 1e-6) ?? values[values.length - 1];
  return [...values].reverse().find((v) => v < current - 1e-6) ?? values[0];
}

const systemDark = () => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;

export function resolvedTheme(s: Settings): "dark" | "light" {
  return s.theme === "system" ? (systemDark() ? "dark" : "light") : s.theme;
}

export function resolvedLang(s: Settings): Lang {
  if (s.language !== "system") return s.language;
  return (navigator.language || "ja").toLowerCase().startsWith("ja") ? "ja" : "en";
}

export function resolvedNotation(s: Settings): Notation {
  if (s.notation !== "auto") return s.notation;
  return resolvedLang(s) === "ja" ? "ja" : "western";
}

let appliedZoom = 1;
let appliedLang = "";

/** Push settings into the document (CSS variables, theme) and the native window. */
export function applySettings(s: Settings) {
  const root = document.documentElement;
  const l = resolvedLang(s);
  setLang(l);
  setNotation(resolvedNotation(s));
  if (l !== appliedLang) {
    appliedLang = l;
    invoke("set_language", { lang: l }).catch(() => {});
  }
  const theme = resolvedTheme(s);
  root.dataset.theme = theme;
  const accent = ACCENTS[s.accent][theme];
  const board = BOARDS[s.board];
  root.style.setProperty("--accent", accent);
  root.style.setProperty("--wood-a", board.a);
  root.style.setProperty("--wood-b", board.b);
  root.style.setProperty("--grid", board.grid);
  root.style.setProperty("--board-frame", board.frame);
  root.style.setProperty("--piece-font", PIECE_FONTS[s.pieceFont].css);
  root.style.setProperty("--font-scale", String(s.fontScale));
  // Title bar follows the app theme ("system" lets Windows decide).
  getCurrentWindow().setTheme(s.theme === "system" ? null : s.theme).catch(() => {});
  if (Math.abs(appliedZoom - s.zoom) > 1e-6) {
    appliedZoom = s.zoom;
    getCurrentWebview().setZoom(s.zoom).catch(() => {});
  }
}

/** Follow OS light/dark changes while the theme is "system". */
export function watchSystemTheme(get: () => Settings) {
  const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
  mq?.addEventListener("change", () => get().theme === "system" && applySettings(get()));
}

export const PieceLabelContext = createContext<PieceLabels>("one");
export const usePieceLabels = () => useContext(PieceLabelContext);
