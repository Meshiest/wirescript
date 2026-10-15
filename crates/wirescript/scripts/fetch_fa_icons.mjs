#!/usr/bin/env node
// Fetch the Font Awesome Free solid icons the Learn series uses (each lesson's
// `icon`, and the BRANCH_ICONS and WIRING_ICONS in
// crates/wasm/playground/learn.js, plus the graph's own marks) into
// crates/wirescript/scripts/fa-icons.json, which gen_learn_tree.mjs reads. Run
// it after naming an icon the file does not have yet.
//
//   node crates/wirescript/scripts/fetch_fa_icons.mjs
//
// Font Awesome Free icons are licensed CC BY 4.0
// (https://fontawesome.com/license/free); the license line is kept in the file.

import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { repoRoot } from './lib.mjs';

const VERSION = '6.5.2';
const BASE = `https://cdn.jsdelivr.net/npm/@fortawesome/fontawesome-free@${VERSION}/svgs/solid/`;
// Marks the graph draws itself, besides the lessons' icons.
const OWN = ['check', 'pepper-hot'];

const { LEARN, BRANCH_ICONS, WIRING_ICONS } = await import(pathToFileURL(join(repoRoot, 'crates', 'wasm', 'playground', 'learn.js')).href);
const out = join(repoRoot, 'crates', 'wirescript', 'scripts', 'fa-icons.json');
let have = {};
try {
  have = JSON.parse(readFileSync(out, 'utf8')).icons;
} catch {
  // No file yet.
}
const names = [...new Set([...OWN, ...LEARN.map((p) => p.icon).filter(Boolean), ...Object.values(BRANCH_ICONS), ...Object.values(WIRING_ICONS)])].sort();
const icons = {};
for (const name of names) {
  if (have[name]) {
    icons[name] = have[name];
    continue;
  }
  const res = await fetch(BASE + name + '.svg');
  if (!res.ok) throw new Error(`fetch_fa_icons: no solid icon "${name}" in Font Awesome Free ${VERSION} (${res.status})`);
  const svg = await res.text();
  // Only the numbers and the path data are kept; nothing from the file is run.
  const box = svg.match(/viewBox="0 0 (\d+) (\d+)"/);
  const path = svg.match(/<path d="([^"]+)"/);
  if (!box || !path || !/^[\d\s.,MmLlHhVvCcSsQqTtAaZz-]+$/.test(path[1])) {
    throw new Error(`fetch_fa_icons: unexpected SVG for "${name}"`);
  }
  icons[name] = { w: Number(box[1]), h: Number(box[2]), d: path[1] };
}
const license = `Font Awesome Free ${VERSION} by @fontawesome - https://fontawesome.com License - https://fontawesome.com/license/free (Icons: CC BY 4.0)`;
writeFileSync(out, JSON.stringify({ license, icons }, null, 1) + '\n');
console.log(`fetch_fa_icons: ${names.length} icons in ${out}`);
