#!/usr/bin/env node
// Where hovers and diagnostics link into the docs: write
// `crates/wirescript/data/doc_links.json`, which the language server embeds.
//
//   node crates/wirescript/scripts/gen_doc_links.mjs           # write
//   node crates/wirescript/scripts/gen_doc_links.mjs --check   # CI gate
//
// Read from the docs:
//   diagnostics  each `WSxxx` code -> the diagnostics.md section whose table
//                lists it (or, for a code with no row, that first names it)
//   builtins     each builtin -> its own heading in builtins.md when it has
//                one (`### SetLocation`), else the section whose table lists
//                it (`| \`sin(x)\` |`)
//   environments each `Environment.<group>` -> its heading in environment.md
// Written here, every anchor checked against the page's headings:
//   keywords, annotations, types, topics
// Paths are the book's (`builtins.html#setlocation`), relative to the docs
// root.

import { readFileSync } from 'fs';
import { join } from 'path';
import { mdLines, repoRoot, slug, writeOrCheck } from './lib.mjs';

const docsDir = join(repoRoot, 'docs', 'src');
const out = join(repoRoot, 'crates', 'wirescript', 'data', 'doc_links.json');

const ANNOTATION_PINS = 'statements.md#left--right--top--bottom-outer-rerouter-pins';
const PRIMITIVE = 'types.md#primitive-types';
const HAND = {
  keywords: {
    if: 'statements.md#if-conditional-statement',
    on: 'statements.md#on-event-handler',
    chip: 'chips.md#named-chips',
    mod: 'chips.md#mod-inline-chips',
    fn: 'statements.md#fn-function',
    is: 'enums.md#is',
  },
  annotations: {
    left: ANNOTATION_PINS,
    right: ANNOTATION_PINS,
    top: ANNOTATION_PINS,
    bottom: ANNOTATION_PINS,
    label: 'statements.md#label-port-display-label',
    color: 'statements.md#color--material-brick-color-and-material',
    material: 'statements.md#color--material-brick-color-and-material',
    closed: 'chips.md#open-and-closed-chips',
    nofold: 'statements.md#nofold-suppress-constant-folding',
    fold: 'statements.md#fold-constant-folding-on-by-default',
    layout: 'statements.md#layoutcode-source-shaped-gate-layout',
    flat: 'statements.md#flat-inline-every-chip',
    sliced: 'statements.md#sliced-loops-sliced',
  },
  types: {
    bool: PRIMITIVE,
    int: PRIMITIVE,
    float: PRIMITIVE,
    string: PRIMITIVE,
    vector: PRIMITIVE,
    rotator: PRIMITIVE,
    quat: PRIMITIVE,
    color: PRIMITIVE,
    entity: 'types.md#object-references--assets',
    character: 'types.md#object-references--assets',
    controller: 'types.md#object-references--assets',
    exec: 'types.md#exec-type',
    zone: 'types.md#zone--teleport-references',
    teleport: 'types.md#zone--teleport-references',
    prefab: 'expressions.md#prefab-references',
    brick: 'types.md#bricks',
    any: 'types.md#any-type',
  },
  topics: {
    constraintClass: 'types.md#constraint-classes-bounds',
    arrayMethods: 'builtins.md#array-methods-exec',
    mapMethods: 'builtins.md#maps-var-m-mapk-v',
    builtinEvents: 'statements.md#triggering-on-built-in-events',
    customEvents: 'builtins.md#custom-events',
    atoms: 'expressions.md#atom-literals',
    assets: 'expressions.md#asset-references',
    prefabs: 'expressions.md#prefab-references',
    environment: 'environment.md#reading-and-writing',
  },
};

/** Every heading of a page, in order, with its anchor (repeats numbered `-1`, `-2`). */
function headings(text) {
  const seen = new Map();
  const list = [];
  for (const { line, index, fence, fenced } of mdLines(text)) {
    if (fence || fenced) continue;
    const m = line.match(/^(#{1,6}) +(.+?)\s*$/);
    if (!m) continue;
    const base = slug(m[2]);
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    list.push({ index, depth: m[1].length, text: m[2], anchor: n ? `${base}-${n}` : base });
  }
  return list;
}

const pages = new Map();
function page(file) {
  if (!pages.has(file)) {
    const text = readFileSync(join(docsDir, file), 'utf8');
    pages.set(file, { text, headings: headings(text) });
  }
  return pages.get(file);
}

/** `file.md#anchor` as the book's `file.html#anchor`, after checking the anchor exists. */
function bookPath(target, where) {
  const [file, anchor] = target.split('#');
  const { headings } = page(file);
  if (anchor && !headings.some((h) => h.anchor === anchor)) {
    throw new Error(`${where}: ${file} has no heading #${anchor}`);
  }
  return file.replace(/\.md$/, '.html') + (anchor ? `#${anchor}` : '');
}

/**
 * Each line of `file` with the anchor of the heading it sits under: the
 * prose lines, and with `fences` also the bodies of fenced blocks, with the
 * block's `info` string.
 */
function* linesUnderHeadings(file, { fences = false } = {}) {
  const { text, headings } = page(file);
  let h = -1;
  for (const { line, index, fence, fenced, info } of mdLines(text)) {
    while (h + 1 < headings.length && headings[h + 1].index <= index) h++;
    if (fence || h < 0 || (fenced && !fences)) continue;
    yield { line, heading: headings[h], fenced, info };
  }
}

function diagnostics() {
  const map = {};
  const rows = [];
  const mentions = [];
  for (const { line, heading } of linesUnderHeadings('diagnostics.md')) {
    const cell = line.match(/^\|([^|]*)\|/);
    if (cell) for (const [code] of cell[1].matchAll(/WSP?\d{3}/g)) rows.push([code, heading]);
    else for (const [code] of line.matchAll(/WSP?\d{3}/g)) mentions.push([code, heading]);
  }
  for (const [code, heading] of [...rows, ...mentions]) {
    if (heading.anchor !== 'contents') map[code] ??= `diagnostics.html#${heading.anchor}`;
  }
  return map;
}

function builtins() {
  const map = {};
  const { headings } = page('builtins.md');
  // A heading that is only names, joined by `/`, names those builtins:
  // `### SetLocation`, `### GetBrickProperty / SetBrickProperty`,
  // `## Clock (Event)`. A `##` heading is a group (`## Bitwise (Pure)`)
  // unless it is an event's.
  for (const h of headings) {
    const m = h.text.match(/^(`?[A-Z]\w*`?(?: \/ `?[A-Z]\w*`?)*)(?: \((\w+)\))?$/);
    if (!m || (h.depth < 3 && m[2] !== 'Event')) continue;
    for (const w of m[1].replace(/`/g, '').split(' / ')) map[w] ??= `builtins.html#${h.anchor}`;
  }
  // Then, in page order, a table row's first cell (`sin(x)`, `v.Dot(o)`,
  // `arr.push(value)`) or a line of a plain signature block, which opens
  // with the call (`controller.GetDisplayName() -> string`). A ```wirescript
  // block is an example: the calls in it are used, not documented, there.
  for (const { line, heading, fenced, info } of linesUnderHeadings('builtins.md', { fences: true })) {
    let names = [];
    if (fenced) {
      if (info === '' || info === 'text') {
        const m = line.match(/^\s*(?:[A-Za-z_]\w*\.)?([A-Za-z_]\w*)\(/);
        if (m) names = [m[1]];
      }
    } else {
      const cell = line.match(/^\|([^|]*)\|/);
      if (cell) names = [...cell[1].matchAll(/`(?:[A-Za-z_]\w*\.)?([A-Za-z_]\w*)\(/g)].map((m) => m[1]);
    }
    for (const name of names) map[name] ??= `builtins.html#${heading.anchor}`;
  }
  // Last, a name inside a longer `###` heading
  // (`Global variants: SendGlobalCustomEvent / on GlobalCustomEvent`).
  for (const h of headings) {
    if (h.depth < 3) continue;
    for (const [w] of h.text.matchAll(/\b[A-Z][a-z]+[A-Z]\w*/g)) map[w] ??= `builtins.html#${h.anchor}`;
  }
  return map;
}

function environments() {
  const map = {};
  for (const h of page('environment.md').headings) {
    const m = h.text.match(/^`Environment\.(\w+)`$/);
    if (m) map[m[1]] = `environment.html#${h.anchor}`;
  }
  return map;
}

function hand() {
  const result = {};
  for (const [kind, entries] of Object.entries(HAND)) {
    result[kind] = {};
    for (const [name, target] of Object.entries(entries)) result[kind][name] = bookPath(target, `${kind}.${name}`);
  }
  return result;
}

const sorted = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
const links = {
  diagnostics: sorted(diagnostics()),
  builtins: sorted(builtins()),
  environments: sorted(environments()),
  ...hand(),
};
const ok = writeOrCheck(out, JSON.stringify(links, null, 2) + '\n', {
  what: 'crates/wirescript/data/doc_links.json',
  fix: 'just doc-links',
});
if (!ok) process.exit(1);
console.log(
  `gen_doc_links: ${Object.keys(links.diagnostics).length} diagnostics, ${Object.keys(links.builtins).length} builtins, ` +
    `${Object.keys(links.environments).length} environments` +
    (process.argv.includes('--check') ? ', up to date' : ''),
);
