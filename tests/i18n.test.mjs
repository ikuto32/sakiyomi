// Translation coverage. Run with: npm run test:i18n
// 1. Every Japanese string literal in src/ is a key of the English dictionary
//    (or listed below as data that is never translated, e.g. piece characters).
// 2. No Japanese JSX text is left outside t().
// 3. Placeholders ({name}) match between Japanese and English.
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { EN } from "../target/i18n-test/i18n.mjs";

const SRC = path.resolve("src");
const JA = /[぀-ヿ㐀-鿿]/;
// Shogi data and proper names shown as-is in every language.
const NOT_UI = new Set([
  "歩", "香", "桂", "銀", "角", "飛", "金", "玉", "王", "と", "杏", "圭", "全", "馬", "龍",
  "歩兵", "香車", "桂馬", "銀将", "角行", "飛車", "金将", "と金", "成香", "成桂", "成銀", "龍馬", "龍王", "王将", "玉将",
  "UD デジタル 教科書体 N-B", "UD デジタル 教科書体 N-R", "日本語", "先読み", "先読み — Sakiyomi",
  "一二三四五六七八九",
]);

const files = [];
(function walk(d) {
  for (const f of fs.readdirSync(d)) {
    const p = path.join(d, f);
    if (fs.statSync(p).isDirectory()) walk(p);
    else if (/\.(ts|tsx)$/.test(f) && f !== "i18n.ts") files.push(p);
  }
})(SRC);

const missing = [];
const rawJsx = [];
let literals = 0;
for (const file of files) {
  const src = fs.readFileSync(file, "utf8").replace(/\/\/.*$|\/\*[\s\S]*?\*\//gm, "");
  const rel = path.relative(SRC, file);
  for (const m of src.matchAll(/"((?:[^"\\\n]|\\.)*)"/g)) {
    const s = JSON.parse(`"${m[1]}"`);
    if (!JA.test(s)) continue;
    literals++;
    if (!(s in EN) && !NOT_UI.has(s)) missing.push(`${rel}: ${s}`);
  }
  for (const m of src.matchAll(/`([^`]*)`/g)) {
    if (JA.test(m[1].replace(/\$\{[^}]*\}/g, ""))) missing.push(`${rel}: template literal \`${m[1]}\``);
  }
  // JSX text, including text on its own line between tags.
  for (const m of src.matchAll(/>([^<>{}"'`();=]+)</g)) {
    if (JA.test(m[1]) && !NOT_UI.has(m[1].trim())) rawJsx.push(`${rel}: ${m[1].trim()}`);
  }
}
assert.ok(literals > 150, `expected many translated literals, found ${literals}`);
assert.deepEqual(missing, [], "Japanese strings without an English entry");
assert.deepEqual(rawJsx, [], "Japanese JSX text outside t()");
for (const [ja, en] of Object.entries(EN)) {
  const ph = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
  assert.equal(ph(ja), ph(en), `placeholders differ: ${ja} / ${en}`);
  assert.ok(!JA.test(en.replace(/[歩龍▲△七六兵王同]/g, "")) || /（|\(/.test(en), `English text still Japanese: ${en}`);
}
console.log(`i18n ok: ${literals} Japanese literals in ${files.length} files, ${Object.keys(EN).length} English entries`);
