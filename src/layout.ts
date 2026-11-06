// Dock layout model (VS Code-like): a tree of splits whose leaves are tab groups.
import { t } from "./i18n";

export type ViewId = "board" | "moves" | "tree" | "graph" | "thinking" | "clocks" | "comment" | "book" | "resources";
export const ALL_VIEWS: ViewId[] = ["board", "moves", "tree", "graph", "thinking", "clocks", "comment", "book", "resources"];
export const VIEW_TITLES: Record<ViewId, string> = {
  board: "盤面",
  moves: "棋譜",
  tree: "分岐ツリー",
  graph: "評価値グラフ",
  thinking: "読み筋",
  clocks: "対局時計",
  comment: "コメント",
  book: "定跡",
  resources: "リソース",
};

export interface Group {
  type: "group";
  id: string;
  tabs: ViewId[];
  active: ViewId;
}
export interface Split {
  type: "split";
  id: string;
  /** row = children side by side, col = stacked vertically */
  dir: "row" | "col";
  children: LayoutNode[];
  /** Fractions summing to 1. */
  sizes: number[];
}
export type LayoutNode = Group | Split;
export type Zone = "center" | "left" | "right" | "top" | "bottom";

let seq = 0;
const uid = (p: string) => `${p}${Date.now().toString(36)}${(seq++).toString(36)}`;

export const group = (tabs: ViewId[], active = tabs[0]): Group => ({ type: "group", id: uid("g"), tabs, active });
export const split = (dir: "row" | "col", children: LayoutNode[], sizes?: number[]): Split => ({
  type: "split",
  id: uid("s"),
  dir,
  children,
  sizes: sizes ?? children.map(() => 1 / children.length),
});

/** Display title of a view (translated). */
export const viewTitle = (v: ViewId) => t(VIEW_TITLES[v]);

export const PRESETS: Record<string, { title: string; make: () => LayoutNode }> = {
  standard: {
    title: "標準",
    make: () =>
      split("col", [
        split("row", [
          split("col", [group(["board"]), group(["graph"])], [0.8, 0.2]),
          split("col", [group(["clocks"]), group(["moves", "tree", "book"]), group(["comment"])], [0.2, 0.62, 0.18]),
        ], [0.74, 0.26]),
        group(["thinking", "resources"]),
      ], [0.8, 0.2]),
  },
  analysis: {
    title: "検討重視",
    make: () =>
      split("row", [
        split("col", [group(["board"]), group(["thinking"])], [0.62, 0.38]),
        split("col", [group(["moves", "tree", "book", "clocks"]), group(["graph", "resources"]), group(["comment"])], [0.5, 0.32, 0.18]),
      ], [0.66, 0.34]),
  },
  board: {
    title: "盤面重視",
    make: () => split("row", [group(["board"]), split("col", [group(["clocks"]), group(["moves", "tree", "book", "graph", "thinking", "comment", "resources"])], [0.22, 0.78])], [0.78, 0.22]),
  },
  compact: {
    title: "コンパクト（4面向け）",
    make: () =>
      split("col", [
        split("row", [group(["board"]), group(["moves", "clocks", "tree", "book", "comment"])], [0.72, 0.28]),
        group(["thinking", "graph", "resources"]),
      ], [0.78, 0.22]),
  },
};

export function defaultLayout(paneCount: number): LayoutNode {
  return (paneCount >= 4 ? PRESETS.compact : PRESETS.standard).make();
}

// ------------------------------------------------------------------ queries

export function groups(node: LayoutNode): Group[] {
  return node.type === "group" ? [node] : node.children.flatMap(groups);
}

export function views(node: LayoutNode): ViewId[] {
  return groups(node).flatMap((g) => g.tabs);
}

export function findGroupOf(node: LayoutNode, view: ViewId): Group | undefined {
  return groups(node).find((g) => g.tabs.includes(view));
}

/** Groups with their share of the total area. */
function areas(node: LayoutNode, share = 1, out: [Group, number][] = []): [Group, number][] {
  if (node.type === "group") out.push([node, share]);
  else node.children.forEach((c, i) => areas(c, share * node.sizes[i], out));
  return out;
}

// ------------------------------------------------------------------ edits (pure)

function normalize(node: LayoutNode | null): LayoutNode | null {
  if (!node) return null;
  if (node.type === "group") return node.tabs.length ? node : null;
  const kids: LayoutNode[] = [];
  const sizes: number[] = [];
  node.children.forEach((c, i) => {
    const n = normalize(c);
    if (!n) return;
    if (n.type === "split" && n.dir === node.dir) {
      n.children.forEach((cc, j) => {
        kids.push(cc);
        sizes.push(node.sizes[i] * n.sizes[j]);
      });
    } else {
      kids.push(n);
      sizes.push(node.sizes[i]);
    }
  });
  if (!kids.length) return null;
  if (kids.length === 1) return kids[0];
  const total = sizes.reduce((a, b) => a + b, 0) || 1;
  return { ...node, children: kids, sizes: sizes.map((s) => s / total) };
}

/** Post-order rewrite (children first), so `f` may wrap a node without being re-applied to it. */
function map(node: LayoutNode, f: (n: LayoutNode) => LayoutNode): LayoutNode {
  if (node.type === "split") return f({ ...node, children: node.children.map((c) => map(c, f)) });
  return f(node);
}

export function removeView(root: LayoutNode, view: ViewId): LayoutNode | null {
  const stripped = map(root, (n) => {
    if (n.type !== "group" || !n.tabs.includes(view)) return n;
    const tabs = n.tabs.filter((t) => t !== view);
    const i = n.tabs.indexOf(view);
    const active = n.active === view ? tabs[Math.min(i, tabs.length - 1)] : n.active;
    return { ...n, tabs, active };
  });
  return normalize(stripped);
}

/** Move (or add) `view` into group `target`: as a tab (center) or by splitting an edge. */
export function placeView(root: LayoutNode, view: ViewId, target: string, zone: Zone, index?: number): LayoutNode {
  const src = findGroupOf(root, view);
  if (src && src.id === target && zone !== "center" && src.tabs.length === 1) return root;
  if (src && src.id === target && zone === "center") {
    // Reorder within the same group.
    const tabs = src.tabs.filter((t) => t !== view);
    const at = index === undefined ? tabs.length : Math.min(index - (src.tabs.indexOf(view) < index ? 1 : 0), tabs.length);
    tabs.splice(Math.max(0, at), 0, view);
    return map(root, (n) => (n.id === target ? { ...src, tabs, active: view } : n));
  }
  const base = src ? removeView(root, view) : root;
  if (!base || !groups(base).some((g) => g.id === target)) return root;
  const placed = map(base, (n) => {
    if (n.id !== target || n.type !== "group") return n;
    if (zone === "center") {
      const tabs = [...n.tabs];
      tabs.splice(index ?? tabs.length, 0, view);
      return { ...n, tabs, active: view };
    }
    const fresh = group([view]);
    const dir = zone === "left" || zone === "right" ? "row" : "col";
    const first = zone === "left" || zone === "top";
    return split(dir, first ? [fresh, n] : [n, fresh], [0.5, 0.5]);
  });
  return normalize(placed) ?? root;
}

/** Wrap the whole layout and add `view` along one edge (like VS Code's panel). */
function placeAtRootEdge(root: LayoutNode, view: ViewId, zone: Exclude<Zone, "center">, share: number): LayoutNode {
  const dir = zone === "left" || zone === "right" ? "row" : "col";
  const first = zone === "left" || zone === "top";
  const g = group([view]);
  return normalize(split(dir, first ? [g, root] : [root, g], first ? [share, 1 - share] : [1 - share, share]))!;
}

/** Bring a hidden view back to a sensible place. */
export function restoreView(root: LayoutNode | null, view: ViewId, paneCount: number): LayoutNode {
  if (!root) return group([view]);
  if (views(root).includes(view)) {
    return map(root, (n) => (n.type === "group" && n.tabs.includes(view) ? { ...n, active: view } : n));
  }
  const near = (v: ViewId) => findGroupOf(root, v);
  const sizeSplit = (g: Group, zone: Exclude<Zone, "center">, share: number) => {
    const r = placeView(root, view, g.id, zone);
    // Give the new group `share` of the space it took from its neighbour.
    const before = zone === "left" || zone === "top";
    return map(r, (n) => {
      if (n.type !== "split") return n;
      const k = n.children.findIndex((c) => c.type === "group" && c.tabs.length === 1 && c.tabs[0] === view);
      const j = before ? k + 1 : k - 1;
      if (k < 0 || j < 0 || j >= n.children.length) return n;
      const sizes = [...n.sizes];
      const total = sizes[k] + sizes[j];
      sizes[k] = total * share;
      sizes[j] = total * (1 - share);
      return { ...n, sizes };
    });
  };
  switch (view) {
    case "resources": {
      const t = near("thinking") ?? near("graph");
      return t ? placeView(root, view, t.id, "center") : placeAtRootEdge(root, view, "bottom", 0.2);
    }
    case "thinking":
      return placeAtRootEdge(root, view, "bottom", paneCount >= 4 ? 0.25 : 0.2);
    case "graph": {
      const b = near("board");
      return b ? sizeSplit(b, "bottom", 0.22) : placeAtRootEdge(root, view, "bottom", 0.2);
    }
    case "clocks": {
      const m = near("moves") ?? near("tree");
      return m ? sizeSplit(m, "top", 0.25) : placeAtRootEdge(root, view, "right", 0.25);
    }
    case "comment": {
      const m = near("moves") ?? near("tree");
      return m ? sizeSplit(m, "bottom", 0.25) : placeAtRootEdge(root, view, "right", 0.25);
    }
    case "book": {
      // Beside the move list as a tab, like the tree.
      const m = near("moves") ?? near("tree") ?? near("comment");
      return m ? placeView(root, view, m.id, "center") : placeAtRootEdge(root, view, "right", 0.26);
    }
    case "moves":
    case "tree": {
      const other = near(view === "moves" ? "tree" : "moves");
      if (other) return placeView(root, view, other.id, "center");
      return placeAtRootEdge(root, view, "right", 0.26);
    }
    case "board": {
      const [largest] = areas(root).sort((a, b) => b[1] - a[1]);
      return placeView(root, view, largest[0].id, "center");
    }
  }
}

export function setSizes(root: LayoutNode, splitId: string, sizes: number[]): LayoutNode {
  return map(root, (n) => (n.id === splitId && n.type === "split" ? { ...n, sizes } : n));
}

export function activate(root: LayoutNode, groupId: string, view: ViewId): LayoutNode {
  return map(root, (n) => (n.id === groupId && n.type === "group" ? { ...n, active: view } : n));
}

/** Validate a stored layout (from localStorage); null if unusable. */
export function sanitize(raw: unknown): LayoutNode | null {
  const seen = new Set<ViewId>();
  const walk = (n: any): LayoutNode | null => {
    if (!n || typeof n !== "object") return null;
    if (n.type === "group" && Array.isArray(n.tabs)) {
      const tabs = n.tabs.filter((t: any) => ALL_VIEWS.includes(t) && !seen.has(t));
      tabs.forEach((t: ViewId) => seen.add(t));
      if (!tabs.length) return null;
      return { type: "group", id: uid("g"), tabs, active: tabs.includes(n.active) ? n.active : tabs[0] };
    }
    if (n.type === "split" && Array.isArray(n.children) && (n.dir === "row" || n.dir === "col")) {
      const kids = n.children.map(walk);
      const sizes = Array.isArray(n.sizes) && n.sizes.length === kids.length ? n.sizes.map(Number) : kids.map(() => 1);
      return {
        type: "split", id: uid("s"), dir: n.dir,
        children: kids.filter(Boolean) as LayoutNode[],
        sizes: sizes.filter((_: number, i: number) => kids[i]).map((s: number) => (isFinite(s) && s > 0 ? s : 0.1)),
      };
    }
    return null;
  };
  return normalize(walk(raw));
}
