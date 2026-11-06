// Property-style checks of the dock layout operations.
// Run with: npm run test:layout (bundles src/layout.ts with esbuild first).
import * as L from "../target/layout-test/layout.mjs";
import assert from "node:assert/strict";

const show = (n) => (n.type === "group" ? `[${n.tabs.join(",")}]` : `${n.dir}(${n.children.map(show).join(" ")})`);
const sum = (n) => (n.type === "group" ? true : Math.abs(n.sizes.reduce((a, b) => a + b, 0) - 1) < 1e-9 && n.children.every(sum));
const valid = (n) => {
  const v = L.views(n);
  assert.equal(new Set(v).size, v.length, "no duplicate views");
  assert.ok(sum(n), "sizes sum to 1");
  const walk = (x) => x.type === "group" ? assert.ok(x.tabs.length && x.tabs.includes(x.active)) : (assert.ok(x.children.length >= 2), x.children.forEach((c) => { if (c.type === "split") assert.notEqual(c.dir, x.dir, "no same-dir nesting"); walk(c); }));
  walk(n);
};

let r = L.PRESETS.standard.make();
valid(r);
const board = () => L.findGroupOf(r, "board");
r = L.placeView(r, "graph", board().id, "right");
valid(r);
console.log("graph → right of board:", show(r));
assert.match(show(r), /row\(\[board\] \[graph\] col/);

r = L.placeView(r, "thinking", L.findGroupOf(r, "moves").id, "center", 0);
valid(r);
assert.deepEqual(L.findGroupOf(r, "moves").tabs.slice(0, 2), ["thinking", "moves"]);
console.log("thinking stacked into moves:", show(r));

// Dropping a single-tab group onto its own edge is a no-op.
assert.equal(L.placeView(r, "graph", L.findGroupOf(r, "graph").id, "left"), r);

// Remove and restore each view; the set of views is preserved.
for (const v of L.ALL_VIEWS) {
  const removed = L.removeView(r, v);
  valid(removed);
  assert.ok(!L.views(removed).includes(v));
  const back = L.restoreView(removed, v, 1);
  valid(back);
  assert.deepEqual([...L.views(back)].sort(), [...L.views(r)].sort(), v);
}

// Random drag sequences never corrupt the tree.
let seed = 1;
const rnd = (n) => ((seed = (seed * 48271) % 2147483647), seed % n);
const zones = ["center", "left", "right", "top", "bottom"];
for (let i = 0; i < 2000; i++) {
  const gs = L.groups(r);
  const views = L.views(r);
  r = L.placeView(r, views[rnd(views.length)], gs[rnd(gs.length)].id, zones[rnd(5)], rnd(3));
  valid(r);
  assert.equal(L.views(r).length, L.ALL_VIEWS.length);
}
// Stored layouts round-trip through sanitize.
const back = L.sanitize(JSON.parse(JSON.stringify(r)));
valid(back);
assert.equal(show(back), show(r));
assert.equal(L.sanitize({ type: "bogus" }), null);
console.log("layout tests passed:", show(r));
