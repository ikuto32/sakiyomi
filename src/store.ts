import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, EngineConfig, JobView, OpenBookView, SessionView, SystemStats, ThinkingView } from "./api";
import { LayoutNode, ViewId, defaultLayout, removeView, restoreView, sanitize, views } from "./layout";
import { Settings, applySettings, loadSettings, saveSettings } from "./settings";
import { t } from "./i18n";

export type Layout = 1 | 2 | 4;

export interface PaneState {
  sessionId: number | null;
  flipped: boolean;
  editing: boolean;
  /** Dock arrangement of this pane's views. */
  dock: LayoutNode;
}

/** Sizes of the multi-view grid (fractions). */
export interface GridSizes {
  two: number[];
  rows: number[];
  top: number[];
  bottom: number[];
}

interface State {
  sessions: Map<number, SessionView>;
  /** session id → engine key → latest thinking */
  thinking: Map<number, Map<number, ThinkingView>>;
  engines: EngineConfig[];
  layout: Layout;
  panes: PaneState[];
  /** Pane that receives keyboard input. Changed only by clicking a pane, never by hover. */
  activePane: number;
  grid: GridSizes;
  toasts: { id: number; text: string; error: boolean }[];
  system: SystemStats | null;
  /** Recent samples for the resource view (oldest first). */
  history: { cpu: number[]; gpu: number[]; engineCpu: number[] };
  palette: boolean;
  settings: Settings;
  settingsOpen: boolean;
  /** Background jobs (conversions, mining) by id. */
  jobs: Map<number, JobView>;
  /** Books opened for the book panel. */
  books: OpenBookView[];
  toolsOpen: boolean;
}

// ------------------------------------------------------------------ persistence

function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}
function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage unavailable */
  }
}
const dockKey = (count: number, index: number) => `dock:${count}:${index}`;
function loadDock(count: number, index: number): LayoutNode {
  return sanitize(load<unknown>(dockKey(count, index), null)) ?? defaultLayout(count);
}

const savedLayout = load<number>("layoutCount", 1);
const initialLayout: Layout = savedLayout === 2 || savedLayout === 4 ? savedLayout : 1;
const newPane = (count: number, index: number, sessionId: number | null = null): PaneState => ({
  sessionId, flipped: false, editing: false, dock: loadDock(count, index),
});

let state: State = {
  sessions: new Map(),
  thinking: new Map(),
  engines: [],
  layout: 1,
  panes: [newPane(1, 0)],
  activePane: 0,
  grid: load<GridSizes>("gridSizes", { two: [0.5, 0.5], rows: [0.5, 0.5], top: [0.5, 0.5], bottom: [0.5, 0.5] }),
  toasts: [],
  system: null,
  history: { cpu: [], gpu: [], engineCpu: [] },
  palette: false,
  settings: loadSettings(),
  settingsOpen: false,
  jobs: new Map(),
  books: [],
  toolsOpen: false,
};

const listeners = new Set<() => void>();
function set(patch: Partial<State>) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}
export function getState() {
  return state;
}
export function useStore<T>(select: (s: State) => T): T {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => select(state),
  );
}

let toastId = 0;
export function toast(text: string, error = false) {
  const id = ++toastId;
  set({ toasts: [...state.toasts, { id, text, error }] });
  setTimeout(() => set({ toasts: state.toasts.filter((t) => t.id !== id) }), error ? 6000 : 3000);
}

/** Run a backend call, reporting errors as a toast. */
export async function run<T>(p: Promise<T>): Promise<T | undefined> {
  try {
    return await p;
  } catch (e) {
    toast(String(e), true);
    return undefined;
  }
}

/** Like `run`, but resolves to whether the call succeeded. */
export async function runOk(p: Promise<unknown>): Promise<boolean> {
  try {
    await p;
    return true;
  } catch (e) {
    toast(String(e), true);
    return false;
  }
}

function putSession(v: SessionView) {
  const sessions = new Map(state.sessions);
  sessions.set(v.id, v);
  set({ sessions });
}

const HISTORY = 120;
const push = (a: number[], x: number) => [...a.slice(Math.max(0, a.length - HISTORY + 1)), x];

export async function init() {
  await listen<SessionView>("shogi://session", (e) => putSession(e.payload));
  await listen<ThinkingView>("shogi://thinking", (e) => {
    const t = e.payload;
    const thinking = new Map(state.thinking);
    const per = new Map(thinking.get(t.session) ?? []);
    per.set(t.key, t);
    thinking.set(t.session, per);
    set({ thinking });
  });
  await listen<number>("shogi://closed", (e) => {
    const sessions = new Map(state.sessions);
    sessions.delete(e.payload);
    const thinking = new Map(state.thinking);
    thinking.delete(e.payload);
    set({ sessions, thinking });
  });
  await listen<SystemStats>("shogi://system", (e) => {
    const s = e.payload;
    const h = state.history;
    set({
      system: s,
      history: {
        cpu: push(h.cpu, s.cpu),
        gpu: push(h.gpu, s.gpus[0]?.util ?? 0),
        engineCpu: push(h.engineCpu, s.engines.reduce((a, x) => a + x.cpu, 0)),
      },
    });
  });
  await listen<JobView>("shogi://job", (e) => {
    const j = e.payload;
    const jobs = new Map(state.jobs);
    const before = jobs.get(j.id);
    jobs.set(j.id, j);
    set({ jobs });
    if (j.done && !before?.done) jobDone(j);
  });
  const [engines, existing, jobs] = await Promise.all([api.listEngines(), api.listSessions(), api.listJobs()]);
  set({ jobs: new Map(jobs.map((j) => [j.id, j])) });
  set({ engines });
  existing.forEach(putSession);
  const first = existing[0] ?? (await api.createSession());
  putSession(first);
  setPane(0, { sessionId: first.id });
  if (initialLayout > 1) await setLayout(initialLayout);
}

function jobDone(j: JobView) {
  if (j.error) {
    toast(j.cancelled ? t("中止しました: {title}", { title: j.title }) : `${j.title}: ${j.error}`, !j.cancelled);
    return;
  }
  if (j.kind === "openBook" && j.result) {
    const b = j.result as OpenBookView;
    set({ books: [...state.books, b] });
    toast(t("定跡を開きました: {name}（{n}局面）", { name: b.name, n: b.positions }));
    return;
  }
  toast(t("完了しました: {title}", { title: j.title }));
}

export function closeBook(id: number) {
  void run(api.closeBook(id));
  set({ books: state.books.filter((b) => b.id !== id) });
}

export async function clearFinishedJobs() {
  if (!(await runOk(api.clearJobs()))) return;
  set({ jobs: new Map([...state.jobs].filter(([, j]) => !j.done)) });
}

export function setToolsOpen(open: boolean) {
  set({ toolsOpen: open });
}

export async function reloadEngines() {
  set({ engines: await api.listEngines() });
}

export function setPane(index: number, patch: Partial<PaneState>) {
  const panes = state.panes.map((p, i) => (i === index ? { ...p, ...patch } : p));
  set({ panes });
  if (patch.dock) save(dockKey(state.layout, index), patch.dock);
}

export function setDock(index: number, dock: LayoutNode) {
  setPane(index, { dock });
}

/** Show or hide a view in a pane (keeps at least one view). */
export function toggleView(index: number, view: ViewId) {
  const pane = state.panes[index];
  if (!pane) return;
  if (views(pane.dock).includes(view)) {
    const next = removeView(pane.dock, view);
    if (next) setDock(index, next);
  } else {
    setDock(index, restoreView(pane.dock, view, state.layout));
  }
}

export function applyDockToAll(index: number) {
  const dock = state.panes[index]?.dock;
  if (!dock) return;
  state.panes.forEach((_, i) => i !== index && setDock(i, sanitize(dock)!));
  toast(t("この配置を全ビューに適用しました"));
}

export function setGrid(patch: Partial<GridSizes>) {
  const grid = { ...state.grid, ...patch };
  set({ grid });
  save("gridSizes", grid);
}

export function updateSettings(patch: Partial<Settings>) {
  const settings = { ...state.settings, ...patch };
  set({ settings });
  saveSettings(settings);
  applySettings(settings);
}

export function setSettingsOpen(open: boolean) {
  if (state.settingsOpen !== open) set({ settingsOpen: open });
}

export function setPalette(open: boolean) {
  if (state.palette !== open) set({ palette: open });
}

export function setActivePane(index: number) {
  if (state.activePane !== index) set({ activePane: index });
}

export async function newSession(): Promise<SessionView | undefined> {
  const v = await run(api.createSession());
  if (v) putSession(v);
  return v;
}

export async function setLayout(layout: Layout) {
  const old = state.panes.slice(0, layout);
  const used = new Set(old.map((p) => p.sessionId));
  const spare = [...state.sessions.keys()].filter((id) => !used.has(id));
  const panes: PaneState[] = [];
  for (let i = 0; i < layout; i++) {
    let id = old[i]?.sessionId ?? spare.shift();
    if (id === undefined) id = (await newSession())?.id;
    panes.push({ ...(old[i] ?? newPane(layout, i)), sessionId: id ?? null, dock: loadDock(layout, i) });
  }
  set({ layout, panes, activePane: Math.min(state.activePane, layout - 1) });
  save("layoutCount", layout);
}

export async function closeSession(id: number) {
  await run(api.closeSession(id));
  const remaining = [...state.sessions.keys()].filter((k) => k !== id);
  let fallback = remaining[0];
  if (fallback === undefined) fallback = (await newSession())?.id ?? 0;
  set({ panes: state.panes.map((p) => (p.sessionId === id ? { ...p, sessionId: fallback, editing: false } : p)) });
}
