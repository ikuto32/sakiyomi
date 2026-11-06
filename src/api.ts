import { invoke } from "@tauri-apps/api/core";

export interface Eval {
  cp: number;
  mate: number | null;
}

/** Game-ending events (KIF names via `specialName` in i18n). */
export type Special =
  | "resign" | "interrupt" | "sennichite" | "checkmate" | "jishogi" | "timeUp"
  | "illegalWin" | "illegalLoss" | "enteringKingWin" | "winByDefault" | "lossByDefault"
  | "noMate" | "draw" | "maxMoves" | "matta";

/** One engine's reading of a position; `eval` is from Sente's point of view. */
export interface EngineEval {
  eval: Eval | null;
  depth: number | null;
  seldepth: number | null;
  nodes: number | null;
  timeMs: number | null;
  pv: string[];
  engine: string | null;
}

/** Readings by the engine playing Sente, the one playing Gote, and the analysis engine. */
export interface NodeAnalysis {
  black: EngineEval | null;
  white: EngineEval | null;
  analysis: EngineEval | null;
}

export interface NodeView {
  parent: number | null;
  children: number[];
  ply: number;
  label: string;
  /** Western notation (☗P-7f). */
  western: string;
  special: Special | null;
  comment: string;
  timeMs: number | null;
  analysis: NodeAnalysis;
  usi: string | null;
  from: number | null;
  to: number | null;
}

export interface BoardView {
  squares: number[];
  hands: number[][];
  side: number;
  sfen: string;
  inCheck: boolean;
}

export interface TimeControl {
  mainMs: number;
  byoyomiMs: number;
  incrementMs: number;
}

export interface GameView {
  players: { kind: string; name: string }[];
  times: TimeControl[];
  remainingMs: number[];
  running: number | null;
  elapsedMs: number;
  over: boolean;
  result: string | null;
  tip: number;
  humanToMove: boolean;
}

export interface SessionView {
  id: number;
  title: string;
  nodes: NodeView[];
  cursor: number;
  board: BoardView;
  legal: string[];
  game: GameView | null;
  analysis: { engine: string; multipv: number; node: number; batch: boolean } | null;
  status: string;
  busy: boolean;
  blackName: string;
  whiteName: string;
  handicap: boolean;
  headers: [string, string][];
  engines: { key: number; role: "black" | "white" | "analysis"; name: string }[];
}

export interface PvLine {
  multipv: number;
  depth: number | null;
  seldepth: number | null;
  nodes: number | null;
  nps: number | null;
  timeMs: number | null;
  hashfull: number | null;
  eval: Eval | null;
  bound: string | null;
  pv: string;
  pvWestern: string;
  pvUsi: string[];
}

export interface ThinkingView {
  session: number;
  key: number;
  role: "black" | "white" | "analysis";
  engine: string;
  node: number;
  searching: boolean;
  lines: PvLine[];
}

export interface GpuStats {
  name: string;
  util: number;
  memUtil: number;
  memUsed: number;
  memTotal: number;
  temp: number | null;
  powerW: number | null;
}

export interface EngineProcStats {
  session: number;
  name: string;
  pid: number;
  /** Share of the whole machine's CPU, 0–100. */
  cpu: number;
  memBytes: number;
  gpuUtil: number | null;
  gpuMem: number | null;
}

export interface SystemStats {
  cpu: number;
  cores: number;
  memUsed: number;
  memTotal: number;
  gpus: GpuStats[];
  gpuError: string | null;
  engines: EngineProcStats[];
}

export interface OptionDef {
  name: string;
  kind: string;
  default: string;
  min: number | null;
  max: number | null;
  vars: string[];
}

export interface EngineConfig {
  id: string;
  name: string;
  path: string;
  author: string;
  usiName: string;
  options: OptionDef[];
  values: Record<string, string>;
}

export interface PlayerConfig {
  kind: "human" | "engine";
  engineId?: string | null;
  name?: string | null;
  nodes?: number | null;
  depth?: number | null;
}

export interface GameConfig {
  players: [PlayerConfig, PlayerConfig];
  times: [TimeControl, TimeControl];
  startSfen: string | null;
  maxMoves: number;
}

// ------------------------------------------------------------------ tools

export type EvalStyle = "shogigui" | "shogihome" | "floodgate" | "none";
export type TextEncoding = "utf8" | "utf8bom" | "sjis";
export type PositionFormat = "sfen" | "usi" | "bod" | "csa" | "hcp" | "psfen";

export interface JobView {
  id: number;
  kind: "convertBooks" | "convertKifu" | "convertDb" | "mine" | "bookStats" | "openBook";
  title: string;
  progress: number | null;
  message: string;
  done: boolean;
  cancelled: boolean;
  error: string | null;
  result: unknown;
}

export interface BookReport {
  positions: number;
  moves: number;
  illegalMoves: number;
  invalidPositions: number;
  duplicatePositions: number;
  unreachable: number;
  needsRoots: number;
  lostFields: string[];
}

export interface BookConvertRequest {
  inputs: string[];
  inputFormat?: string | null;
  output: string;
  outputFormat: "spg" | "spgt" | "spgm" | "db" | "sbk" | "gikou";
  merge: "sum" | "keepFirst" | "preferSecond";
  minCount?: number | null;
  maxValueDrop?: number | null;
  maxPly?: number | null;
  side?: 0 | 1 | null;
  topMoves?: number | null;
  backUp: boolean;
  sbkEvalsFromValues: boolean;
  gikouRoots: string[];
  rootsFrom?: string | null;
  /** SPG output merging several inputs: leave out PROV (each source's contribution). Off by default. */
  noProv?: boolean;
  overwrite: boolean;
}

export interface KifuConvertRequest {
  inputs: string[];
  outDir: string;
  format: "spg" | "spgt" | "kif" | "ki2" | "csa" | "jkf" | "usi";
  encoding: TextEncoding;
  evalStyle: EvalStyle;
  overwrite: boolean;
}

export interface DbConvertRequest {
  input: string;
  inputFormat?: string | null;
  output: string;
  outputFormat: "spg" | "spgt" | "spgm" | "hcpe" | "psv" | "hcpe3" | "sfen" | "csa";
  skipWithoutEval: boolean;
  minPly: number;
  overwrite: boolean;
}

export interface MineRequest {
  bookIn: string | null;
  rawOut: string;
  bookOut: string | null;
  workers: number;
  roots: string[];
  nodes: number;
  multipv: number;
  multipvDelta: number;
  evalDiff: number;
  maxPly: number;
  leavesPerRound: number;
  rounds: number;
  black: boolean;
  white: boolean;
}

export interface MinerProgress {
  round: number;
  searched: number;
  queued: number;
  positions: number;
  errors: number;
}

export interface OpenBookView {
  id: number;
  path: string;
  name: string;
  positions: number;
  report: BookReport;
}

export interface BookMoveView {
  usi: string;
  label: string;
  value: number | null;
  depth: number | null;
  count: number;
  share: number;
  mark: "none" | "forced" | "good" | "bad" | "blunder";
}

export interface BookProbeView {
  found: boolean;
  flipped: boolean;
  moves: BookMoveView[];
  comment: string;
}

// Rust EngineConfig uses snake_case for `usi_name`.
type RawEngine = Omit<EngineConfig, "usiName"> & { usi_name: string };
const engine = (e: RawEngine): EngineConfig => ({ ...e, usiName: e.usi_name });

export const api = {
  listEngines: async () => (await invoke<RawEngine[]>("list_engines")).map(engine),
  registerEngine: async (path: string) => engine(await invoke<RawEngine>("register_engine", { path })),
  refreshEngine: async (id: string) => engine(await invoke<RawEngine>("refresh_engine", { id })),
  updateEngine: async (id: string, name: string | null, values: Record<string, string>) =>
    engine(await invoke<RawEngine>("update_engine", { id, name, values })),
  removeEngine: (id: string) => invoke<void>("remove_engine", { id }),

  createSession: (title?: string) => invoke<SessionView>("create_session", { title: title ?? null }),
  closeSession: (id: number) => invoke<void>("close_session", { id }),
  listSessions: () => invoke<SessionView[]>("list_sessions"),
  thinking: (id: number) => invoke<ThinkingView[]>("thinking", { id }),
  setTitle: (id: number, title: string) => invoke<void>("set_title", { id, title }),
  setCursor: (id: number, node: number) => invoke<void>("set_cursor", { id, node }),
  makeMove: (id: number, usi: string) => invoke<void>("make_move", { id, usi }),
  newGame: (id: number, config: GameConfig) => invoke<void>("new_game", { id, config }),
  stopGame: (id: number) => invoke<void>("stop_game", { id }),
  resign: (id: number) => invoke<void>("resign", { id }),
  moveNow: (id: number) => invoke<void>("move_now", { id }),
  startAnalysis: (id: number, engineId: string, multipv: number, batchMs: number | null) =>
    invoke<void>("start_analysis", { id, engineId, multipv, batchMs }),
  stopAnalysis: (id: number) => invoke<void>("stop_analysis", { id }),
  /** Opens the first record here; further records of an SPG file open in new sessions. */
  loadFile: (id: number, path: string) => invoke<{ opened: number; notice: string | null }>("load_file", { id, path }),
  saveFile: (id: number, path: string, format: string, encoding: TextEncoding, evalStyle: EvalStyle) =>
    invoke<void>("save_file", { id, path, format, encoding, evalStyle }),
  importText: (id: number, text: string) => invoke<void>("import_text", { id, text }),
  exportText: (id: number, what: "kif" | "ki2" | "csa" | "jkf" | "sfen" | "usi" | "bod" | "spgt", evalStyle: EvalStyle = "none") =>
    invoke<string>("export_text", { id, what, evalStyle }),
  setPosition: (id: number, sfen: string) => invoke<void>("set_position", { id, sfen }),
  deleteNode: (id: number, node: number) => invoke<void>("delete_node", { id, node }),
  promoteVariation: (id: number, node: number) => invoke<void>("promote_variation", { id, node }),
  setComment: (id: number, node: number, text: string) => invoke<void>("set_comment", { id, node, text }),
  setHeader: (id: number, key: string, value: string) => invoke<void>("set_header", { id, key, value }),

  jobConvertBooks: (req: BookConvertRequest) => invoke<number>("job_convert_books", { req }),
  jobConvertKifu: (req: KifuConvertRequest) => invoke<number>("job_convert_kifu", { req }),
  jobConvertDb: (req: DbConvertRequest) => invoke<number>("job_convert_db", { req }),
  jobMine: (engineId: string, req: MineRequest) => invoke<number>("job_mine", { engineId, req }),
  jobBookStats: (path: string) => invoke<number>("job_book_stats", { path }),
  jobOpenBook: (path: string) => invoke<number>("job_open_book", { path }),
  closeBook: (id: number) => invoke<void>("close_book", { id }),
  probeBook: (id: number, sfen: string) => invoke<BookProbeView>("probe_book", { id, sfen }),
  convertPosition: (text: string, from: PositionFormat | null, to: PositionFormat) => invoke<string>("convert_position", { text, from, to }),
  listJobs: () => invoke<JobView[]>("list_jobs"),
  cancelJob: (id: number) => invoke<void>("cancel_job", { id }),
  clearJobs: () => invoke<void>("clear_jobs"),
};
