#!/usr/bin/env node
// Write the Learn series' tree into `docs/src/learn/README.md`, between its
// `<!-- learn-tree -->` markers, from the page list in
// `crates/wasm/playground/learn.js`: a graph of the lessons and what each
// needs, then one numbered list per branch, with an "after" note wherever a
// lesson needs one from elsewhere in the tree. Lesson icons come from
// fa-icons.json (see fetch_fa_icons.mjs).
//
// It also writes the links at the top of every Learn page, between
// `<!-- learn-links -->` markers under the title: the pages it builds on, the
// lessons that build on it, and the parts that use it; and the icons the
// book's sidebar shows beside each Learn page and branch, and the wiring
// diagrams draw, into docs/assets/learn-icons.js. It checks each page's
// ```wiring diagrams too.
//
//   node crates/wirescript/scripts/gen_learn_tree.mjs           # write
//   node crates/wirescript/scripts/gen_learn_tree.mjs --check   # CI gate

import { readFileSync } from 'node:fs';
import { join, posix } from 'node:path';
import { pathToFileURL } from 'node:url';
import { isMain, repoRoot, writeOrCheck } from './lib.mjs';

const OPEN = '<!-- learn-tree -->';
const CLOSE = '<!-- /learn-tree -->';
const PARTS_OPEN = '<!-- parts-catalog -->';
const PARTS_CLOSE = '<!-- /parts-catalog -->';
// The Parts catalog page. Entries flagged `part` in learn.js are ready-made
// circuits, not lessons: they stay out of the graph and the lesson list and
// are listed on this page instead.
const CATALOG = 'docs/learn/parts/README.md';
const LINKS_OPEN = '<!-- learn-links -->';
const LINKS_CLOSE = '<!-- /learn-links -->';

/** A page's link target relative to `docs/src/learn/`. */
const linkOf = (page) => page.file.replace(/^docs\/learn\//, '');

/**
 * The tree as markdown: a heading and a numbered list per branch, branches in
 * the order their first page appears. A lesson notes what it needs only when
 * that is not the lesson right before it.
 */
export function renderTree(pages) {
  const byId = new Map(pages.map((p) => [p.id, p]));
  const branches = [];
  for (const p of pages) {
    let branch = branches.find((b) => b.name === p.branch);
    if (!branch) branches.push((branch = { name: p.branch, pages: [] }));
    branch.pages.push(p);
  }
  const out = ['<div class="learn-tree">', ''];
  for (const branch of branches) {
    out.push(`### ${branch.name}`, '');
    branch.pages.forEach((p, i) => {
      const previous = branch.pages[i - 1]?.id;
      const elsewhere = (p.requires || []).filter((id) => id !== previous);
      for (const id of elsewhere) {
        if (!byId.has(id)) throw new Error(`learn.js: ${p.id} requires unknown node ${id}`);
      }
      const after = elsewhere.map((id) => `[${byId.get(id).title}](${linkOf(byId.get(id))})`);
      const admin = p.admin ? ' (needs admin to load)' : '';
      out.push(`${i + 1}. [${p.title}](${linkOf(p)})${after.length ? ` (after ${after.join(', ')})` : ''}${admin}`);
    });
    out.push('');
  }
  out.push('</div>');
  return out.join('\n');
}

// Graph geometry, in SVG units. A lesson is a square holding its icon, with
// its title underneath; a column is one cell wide.
const SQ = 68;
const ICON = 32;
// The icon of a lesson that names none.
const DEFAULT_ICON = 'code';
const CELL_W = 176;
const TITLE_GAP = 6;
const TITLE_H = 36;
const ROW_GAP = 18;
const PITCH = SQ + TITLE_GAP + TITLE_H + ROW_GAP;
// A branch's name sits just above its first lesson.
const LABEL_H = 28;
// Branches that would start level with each other step down by this much,
// the farther from their entry the lower, so their starts do not line up.
const STAGGER = 22;
const PAD = 24;
// The line into a branch leaves its entry lesson's side, runs down the gap
// beside it to `BUS` below the entry's title, across, and drops into the
// branch's name at 35 degrees (rise over run).
const BUS = 10;
const DROP = 14;
const SLOPE = Math.tan((35 * Math.PI) / 180);
// Lanes: lines from different entries down one gap sit `LANE_DX` apart, and
// across the strip above a band `LANE_DY` apart, below `STRIP_PAD`.
const LANE_DX = 5;
const LANE_DY = 8;
const STRIP_PAD = 12;
const BAND_GAP = 16;
// Columns per band: past this the graph wraps into another band below.
export const MAX_COLUMNS = 7;
const round = (v) => Math.round(v * 10) / 10;

const xml = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/** A branch's color: hues spread by the golden angle, in branch order. */
export function branchColor(i) {
  return `hsl(${round((215 + i * 137.508) % 360)}, 55%, 45%)`;
}

let faIcons = null;
/** Font Awesome Free icons, fetched into fa-icons.json by fetch_fa_icons.mjs. */
function icons() {
  if (!faIcons) faIcons = JSON.parse(readFileSync(join(repoRoot, 'crates', 'wirescript', 'scripts', 'fa-icons.json'), 'utf8'));
  return faIcons;
}

/** An icon `size` across, centered on (cx, cy). */
function icon(name, cx, cy, size, cls) {
  const i = icons().icons[name];
  if (!i) throw new Error(`gen_learn_tree: no icon "${name}" in fa-icons.json; run crates/wirescript/scripts/fetch_fa_icons.mjs`);
  return (
    `<svg class="${cls}" x="${round(cx - size / 2)}" y="${round(cy - size / 2)}" width="${size}" height="${size}" ` +
    `viewBox="0 0 ${i.w} ${i.h}"><path d="${i.d}"/></svg>`
  );
}

/**
 * Where each lesson sits in the graph. Branches fill bands of at most
 * `maxColumns` columns, split evenly; each branch is a column of its lessons
 * in reading order. A branch's entry is the requirement of its first lesson
 * that sits lowest. With its entry in the same band, the branch starts just
 * below that lesson, in a column just right of the entry's (deeper entries
 * nearer, so the line from a higher entry passes above them); otherwise it
 * starts at the top of its band, its column ordered by where its entry is.
 * Every entry gets its own lane down the gaps between columns and its own
 * lane across the strip above each band, so lines never run on top of each
 * other.
 * Returns `{ branches, perBand, bands, nodes }`: each branch `{ name, color,
 * pages, band, slot, labelTop, entry, route }`, and `nodes` maps id ->
 * `{ page, branch, slot, top }` (`top` is the square's top edge).
 */
export function layoutGraph(pages, maxColumns = MAX_COLUMNS) {
  const ids = new Set(pages.map((p) => p.id));
  const seenIds = new Set();
  const branches = [];
  for (const p of pages) {
    for (const r of p.requires || []) {
      if (!ids.has(r)) throw new Error(`learn.js: ${p.id} requires unknown node ${r}`);
      if (!seenIds.has(r)) throw new Error(`learn.js: ${p.id} must come after its requirement ${r}`);
    }
    seenIds.add(p.id);
    let branch = branches.find((b) => b.name === p.branch);
    if (!branch) branches.push((branch = { name: p.branch, color: branchColor(branches.length), pages: [] }));
    branch.pages.push(p);
  }
  const bands = Math.max(1, Math.ceil(branches.length / maxColumns));
  const perBand = Math.ceil(branches.length / bands);
  // An odd number of columns, so a band can sit centered on its middle one.
  const width = perBand % 2 ? perBand : perBand + 1;
  branches.forEach((b, i) => (b.band = Math.floor(i / perBand)));
  const branchOf = new Map(branches.flatMap((b) => b.pages.map((p) => [p.id, b])));

  const nodes = new Map();
  const left = (slot) => PAD + slot * CELL_W;
  const cx = (slot) => left(slot) + CELL_W / 2;
  const titleBottom = (n) => n.top + SQ + TITLE_GAP + TITLE_H;
  const deepest = (list) => list.reduce((a, n) => (a && a.top >= n.top ? a : n), null);
  const lanes = new Map(); // gap -> entry ids with a line down it
  const laneX = (gap, id) => {
    const list = lanes.get(gap) || [];
    if (!list.includes(id)) list.push(id);
    lanes.set(gap, list);
    const i = list.indexOf(id);
    return left(gap) + (i % 2 ? 1 : -1) * Math.ceil(i / 2) * LANE_DX;
  };
  const place = (b, labelTop) => {
    b.labelTop = labelTop;
    b.pages.forEach((p, i) => nodes.set(p.id, { page: p, branch: b, slot: 0, top: labelTop + LABEL_H + i * PITCH }));
  };

  let bandTop = PAD;
  for (let band = 0; band < bands; band++) {
    const inBand = branches.filter((b) => b.band === band);
    const reqs = (b) => (b.pages[0].requires || []).filter((r) => branchOf.get(r) !== b);
    for (const b of inBand) {
      b.hangs = reqs(b).some((r) => branchOf.get(r).band === band);
      b.entry = b.hangs ? null : deepest(reqs(b).map((r) => nodes.get(r)));
    }
    // Each entry from an earlier band gets a lane across the strip above.
    const crossing = [...new Set(inBand.map((b) => b.entry).filter(Boolean))];
    crossing.sort((a, c) => a.slot - c.slot || a.top - c.top);
    const strip = crossing.length ? STRIP_PAD + crossing.length * LANE_DY + DROP : 0;
    for (const b of inBand) if (!b.hangs) place(b, bandTop + strip);
    // A branch hanging from this band's lessons starts below its entry
    // (whose branch always comes earlier).
    for (const b of inBand) {
      if (!b.hangs) continue;
      b.entry = deepest(reqs(b).filter((r) => branchOf.get(r).band === band).map((r) => nodes.get(r)));
      place(b, titleBottom(b.entry) + BUS + DROP + 2);
    }
    // Slots: the columns starting at the top, by where their entry comes
    // from, each followed by the columns hanging from it, deepest entry first.
    // A band that starts from nothing (Basics) sits in the middle, with its
    // hanging columns alternating right and left of it, deepest nearest.
    // The band is centered either way.
    const hanging = (b) =>
      inBand
        .filter((d) => d.hangs && d.entry.branch === b)
        .sort((d1, d2) => d2.entry.top - d1.entry.top || inBand.indexOf(d1) - inBand.indexOf(d2));
    const subtree = (b) => [b, ...hanging(b).flatMap(subtree)];
    const from = (b) => (b.entry ? b.entry.slot + 1 : 0);
    const tops = inBand.filter((b) => !b.hangs).sort((a, c) => from(a) - from(c) || inBand.indexOf(a) - inBand.indexOf(c));
    let order;
    let middle = null;
    if (tops.length === 1 && !tops[0].entry) {
      middle = tops[0];
      const right = [];
      const outward = [];
      hanging(middle).forEach((d, i) => (i % 2 ? outward : right).push(...subtree(d)));
      order = [...outward.reverse(), middle, ...right];
    } else {
      order = tops.flatMap(subtree);
    }
    const offset = middle ? (width - 1) / 2 - order.indexOf(middle) : Math.floor((width - order.length) / 2);
    order.forEach((b, i) => {
      b.slot = Math.max(0, offset) + i;
      for (const p of b.pages) nodes.get(p.id).slot = b.slot;
    });
    // Stagger: among branches starting level with each other, the ones
    // farther from their entry start lower. Placing again in the same order
    // carries a moved column down to the columns hanging from it.
    const start = (b) => (b.hangs ? titleBottom(b.entry) + BUS + DROP + 2 : bandTop + strip);
    for (const b of [...inBand.filter((d) => !d.hangs), ...inBand.filter((d) => d.hangs)]) {
      const level = inBand.filter((d) => Math.abs(start(d) - start(b)) < 1 && (d.hangs ? d.entry.page.id : 'top') === (b.hangs ? b.entry.page.id : 'top'));
      const away = (d) => (d.entry ? Math.abs(d.slot - d.entry.slot) : d.slot);
      level.sort((d1, d2) => away(d1) - away(d2) || d1.slot - d2.slot);
      const slots = b.pages.map((p) => nodes.get(p.id).slot);
      place(b, start(b) + level.indexOf(b) * STAGGER);
      b.pages.forEach((p, i) => (nodes.get(p.id).slot = slots[i]));
    }

    // Routes: out of the entry's side into a lane in the gap toward the
    // branch, down to just under the entry's title (in this band) or to the
    // entry's lane in the strip, across, and down at 35 degrees into the name.
    for (const b of inBand) {
      if (!b.entry) continue;
      const e = b.entry;
      const tx = cx(b.slot);
      const dir = b.slot >= e.slot ? 1 : -1;
      const gx = laneX(dir > 0 ? e.slot + 1 : e.slot, e.page.id);
      const sy = e.top + SQ / 2;
      const across = b.hangs ? titleBottom(e) + BUS : bandTop + STRIP_PAD + crossing.indexOf(e) * LANE_DY;
      const end = b.labelTop - 2;
      const toward = Math.sign(tx - gx) || 1;
      const sx = cx(e.slot) + dir * (SQ / 2);
      b.route =
        `M${sx},${round(sy)} L${round(gx)},${round(sy)} L${round(gx)},${round(across)} ` +
        `L${round(tx - (toward * (end - across)) / SLOPE)},${round(across)} L${tx},${round(end)}`;
    }
    bandTop = Math.max(...inBand.map((b) => b.labelTop + LABEL_H + b.pages.length * PITCH - ROW_GAP)) + BAND_GAP;
  }
  return { branches, perBand, bands, width, nodes };
}

/** A title split into at most two lines that fit under a square. */
function titleLines(title) {
  if (title.length <= 18) return [title];
  const middle = title.length / 2;
  let best = -1;
  for (let i = 0; i < title.length; i++) {
    if (title[i] === ' ' && (best < 0 || Math.abs(i - middle) < Math.abs(best - middle))) best = i;
  }
  return best < 0 ? [title] : [title.slice(0, best), title.slice(best + 1)];
}

/**
 * The lesson a lesson builds on from another branch, beyond the line drawn
 * into its column: shown as a small icon on its corner. A page's own `badge`
 * (a lesson id) wins.
 */
function badgeOf(n, nodes) {
  const p = n.page;
  if (p.badge) return nodes.get(p.badge) || null;
  const drawn = n.branch.pages[0] === p ? n.branch.entry?.page.id : null;
  return (p.requires || []).map((r) => nodes.get(r)).find((r) => r && r.branch !== n.branch && r.page.id !== drawn) || null;
}

/**
 * The lessons as an SVG graph. Each lesson is a square in its branch's color
 * with its icon, its title underneath, peppers for its difficulty, a corner
 * icon for a lesson it builds on from another branch, and a check once it is
 * done. A line runs down each branch, and one into each branch from the
 * lesson it starts after. Each lesson's full requirements are in its tooltip
 * and in the list below the graph.
 */
export function renderGraph(pages) {
  const { branches, width: columns, nodes } = layoutGraph(pages);
  const cx = (slot) => PAD + slot * CELL_W + CELL_W / 2;
  const maxBottom = Math.max(...[...nodes.values()].map((n) => n.top + SQ + TITLE_GAP + TITLE_H));
  const width = PAD * 2 + columns * CELL_W;
  const height = maxBottom + PAD;

  const edge = (d, from) =>
    `<path class="edge" d="${d}" style="stroke: ${from.color}" marker-end="url(#learn-arrow-${branches.indexOf(from)})"/>`;
  const out = [
    `<svg class="learn-graph" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" role="img" aria-label="Lesson tree">`,
    `<!-- Icons: ${xml(icons().license)} -->`,
    '<style>',
    '.learn-graph text { font: 14px sans-serif; fill: var(--fg); }',
    // An outline in the page color, so a line passing behind a title or a
    // branch name runs under it.
    '.learn-graph text { paint-order: stroke; stroke: var(--bg); stroke-width: 5px; stroke-linejoin: round; }',
    '.learn-graph a, .content .learn-graph a:hover { text-decoration: none; }',
    '.learn-graph .branch { font-size: 18px; font-weight: bold; }',
    // Solid, not translucent: lines sharing a stretch draw over each other.
    // Each line takes the color of the branch it comes from.
    '.learn-graph .edge { fill: none; stroke-width: 1.5; }',
    '.learn-graph .sq { fill: var(--c); stroke: var(--fg); stroke-opacity: 0.25; }',
    '.learn-graph .learn-node:hover .sq { filter: brightness(1.2); }',
    '.learn-graph .icon { fill: #fff; }',
    '.learn-graph .pepper { fill: #ffd54a; }',
    '.learn-graph .badge circle { fill: var(--bg); stroke: var(--fg); stroke-opacity: 0.4; }',
    '.learn-graph .learn-node.learn-done .sq { fill: #fff; }',
    '.learn-graph .learn-node.learn-done .icon { fill: var(--c); }',
    '.learn-graph .learn-node.learn-done .pepper { fill: #e8590c; }',
    '.learn-graph .done { display: none; }',
    '.learn-graph .learn-node.learn-done .done { display: inline; }',
    '.learn-graph .done circle { fill: #2ea043; }',
    '.learn-graph .done svg { fill: #fff; }',
    '</style>',
    // One arrowhead per branch, in its color (a marker cannot take the
    // color of the line it ends).
    '<defs>' +
      branches
        .map(
          (b, i) =>
            `<marker id="learn-arrow-${i}" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">` +
            `<path d="M0,0 L8,4 L0,8 z" style="fill: ${b.color}"/></marker>`,
        )
        .join('') +
      '</defs>',
  ];
  for (const b of branches) {
    if (b.route) out.push(edge(b.route, b.entry.branch));
    out.push(`<text class="branch" x="${cx(b.slot)}" y="${b.labelTop + 20}" text-anchor="middle" style="fill: ${b.color}">${xml(b.name)}</text>`);
  }
  // One straight arrow from each lesson to the next in its branch, running
  // behind the title (its outline hides the line there).
  for (const b of branches) {
    b.pages.slice(1).forEach((p, i) => {
      const from = nodes.get(b.pages[i].id);
      const to = nodes.get(p.id);
      const x = cx(b.slot);
      out.push(edge(`M${x},${from.top + SQ} L${x},${to.top - 2}`, b));
    });
  }
  for (const n of nodes.values()) {
    const p = n.page;
    const x = cx(n.slot);
    const sl = x - SQ / 2;
    const href = linkOf(p).replace(/\.md$/, '.html');
    const cls = `learn-node${p.admin ? ' admin' : ''}`;
    const needs = (p.requires || []).map((r) => nodes.get(r).page.title);
    const level = Math.min(3, Math.max(0, p.level || 0));
    const title = [
      p.title,
      p.admin ? '(needs admin to load)' : '',
      level ? `difficulty ${level} of 3` : '',
      needs.length ? `after ${needs.join(', ')}` : '',
    ]
      .filter(Boolean)
      .join(' - ');
    const lines = titleLines(p.title);
    const label = lines.map((line, i) => `<tspan x="${x}" y="${n.top + SQ + TITLE_GAP + 13 + i * 17}">${xml(line)}</tspan>`).join('');
    const peppers = Array.from({ length: level }, (_, i) =>
      icon('pepper-hot', sl + 11 + i * 14, n.top + SQ - 11, 14, 'pepper'),
    ).join('');
    const badge = badgeOf(n, nodes);
    const badgeSvg = badge
      ? `<g class="badge"><circle cx="${sl + SQ - 2}" cy="${n.top + SQ - 2}" r="12"/>` +
        icon(badge.page.icon || DEFAULT_ICON, sl + SQ - 2, n.top + SQ - 2, 13, 'mark').replace('<svg ', `<svg style="fill: ${badge.branch.color}" `) +
        '</g>'
      : '';
    out.push(
      `<a class="${cls}" href="${xml(href)}" style="--c: ${n.branch.color}"><title>${xml(title)}</title>` +
        `<rect class="sq" x="${sl}" y="${n.top}" width="${SQ}" height="${SQ}" rx="9"/>` +
        icon(p.icon || DEFAULT_ICON, x, n.top + SQ / 2, ICON, 'icon') +
        peppers +
        badgeSvg +
        `<g class="done"><circle cx="${sl + SQ - 8}" cy="${n.top + 8}" r="13"/>${icon('check', sl + SQ - 8, n.top + 8, 14, 'mark')}</g>` +
        `<text text-anchor="middle">${label}</text></a>`,
    );
  }
  out.push('</svg>');
  return out.join('\n');
}

/**
 * The Learn page's tree: the graph (scrolling sideways when the branches are
 * wider than the page), then every lesson as a list, which is also what a
 * screen reader reads.
 */
export function renderLearn(pages) {
  pages = pages.filter((p) => !p.part);
  const list = renderTree(pages).replace(/^<div class="learn-tree">\n\n/, '').replace(/\n<\/div>$/, '');
  // The graph may grow wider than the text column, up to the window, centered
  // on it; past that it scrolls sideways.
  // A page marked `graph: false` (an alternative way in, not a step) is in the
  // list but not drawn.
  const graph = renderGraph(pages.filter((p) => p.graph !== false));
  const width = graph.match(/ width="(\d+)"/)[1];
  const wrap =
    `<div style="overflow-x:auto;width:min(100vw - 2em, var(--learn-graph-width, ${width}px));` +
    'position:relative;left:50%;transform:translateX(-50%)">';
  return ['<div class="learn-tree">', wrap, graph, '</div>', '', list, '</div>'].join('\n');
}

/**
 * The Parts catalog: one table per part branch, each part with what it uses
 * (`summary`) and the lessons it builds on, linked relative to the catalog.
 */
export function renderCatalog(pages) {
  const byId = new Map(pages.map((p) => [p.id, p]));
  const fromCatalog = (page) => {
    const target = linkOf(page);
    return target.startsWith('parts/') ? target.slice('parts/'.length) : `../${target}`;
  };
  const branches = [];
  for (const p of pages) {
    if (!p.part || p.file === CATALOG) continue;
    let branch = branches.find((b) => b.name === p.branch);
    if (!branch) branches.push((branch = { name: p.branch, parts: [] }));
    branch.parts.push(p);
  }
  const out = [];
  for (const branch of branches) {
    out.push(`### ${branch.name}`, '', '| Part | Uses | Builds on |', '|---|---|---|');
    for (const p of branch.parts) {
      const needs = (p.requires || []).map((id) => {
        const r = byId.get(id);
        if (!r) throw new Error(`learn.js: ${p.id} requires unknown node ${id}`);
        return `[${r.title}](${fromCatalog(r)})`;
      });
      const admin = p.admin ? ' (needs admin to load)' : '';
      out.push(`| [${p.title}](${fromCatalog(p)})${admin} | ${p.summary || ''} | ${needs.join(', ')} |`);
    }
    out.push('');
  }
  return out.join('\n').trimEnd();
}

/**
 * The links at the top of `page`: what it builds on, the lessons that build on
 * it, and the parts that use it, each linked relative to the page. Empty when
 * there is nothing to link.
 */
export function renderLinks(page, pages) {
  const byId = new Map(pages.map((p) => [p.id, p]));
  const here = posix.dirname(linkOf(page));
  const link = (p) => `[${p.title}](${posix.relative(here, linkOf(p))})`;
  const before = (page.requires || []).map((id) => {
    const r = byId.get(id);
    if (!r) throw new Error(`learn.js: ${page.id} requires unknown node ${id}`);
    return r;
  });
  const after = pages.filter((p) => p.file !== CATALOG && (p.requires || []).includes(page.id));
  const rows = [
    ['Before this', before],
    ['Next steps', after.filter((p) => !p.part)],
    ['Parts that use this', after.filter((p) => p.part)],
  ].filter(([, list]) => list.length);
  if (!rows.length) return '';
  const lines = rows.map(([label, list]) => `**${label}:** ${list.map(link).join(', ')}`);
  return ['<div class="learn-links">', '', lines.join('\n\n'), '', '</div>'].join('\n');
}

/**
 * `text` with its links block set to `block`: replaced where the markers are,
 * else put under the page's `#` title; an empty block removes the markers.
 */
export function withLinks(text, block) {
  const eol = text.includes('\r\n') ? '\r\n' : '\n';
  let body = text.replace(/\r\n/g, '\n');
  const start = body.indexOf(LINKS_OPEN);
  if (start !== -1) {
    const end = body.indexOf(LINKS_CLOSE, start);
    if (end === -1) throw new Error(`unterminated ${LINKS_OPEN} block`);
    body = body.slice(0, start) + body.slice(end + LINKS_CLOSE.length).replace(/^\n+/, '');
  }
  if (block) {
    const title = body.match(/^# .*\n+/);
    if (!title) throw new Error('the page needs a `#` title on its first line');
    const at = title[0].length;
    body = body.slice(0, at) + [LINKS_OPEN, block, LINKS_CLOSE, '', ''].join('\n') + body.slice(at);
  }
  return body.split('\n').join(eol);
}

/**
 * The book's icons as a script (docs/assets/learn-icons.js): each Learn page's
 * icon by its path in the book (`learn/random/dice`) and each branch's by name,
 * which learn-nav.js puts beside the sidebar's entries; each wiring kind's,
 * which ws-wiring.js draws; and the shapes of just those icons.
 */
export function renderBookIcons(pages, branchIcons = {}, wiringIcons = {}) {
  const shapes = {};
  const use = (name) => {
    const i = icons().icons[name];
    if (!i) throw new Error(`gen_learn_tree: no icon "${name}" in fa-icons.json; run crates/wirescript/scripts/fetch_fa_icons.mjs`);
    shapes[name] = [i.w, i.h, i.d];
    return name;
  };
  const byPage = {};
  for (const p of pages) {
    if (!p.icon) continue;
    const path = p.file.replace(/^docs\//, '').replace(/\.md$/, '').replace(/(^|\/)README$/, '$1index');
    byPage[path] = use(p.icon);
  }
  const byBranch = {};
  for (const [branch, name] of Object.entries(branchIcons)) byBranch[branch] = use(name);
  const wiring = {};
  for (const [kind, name] of Object.entries(wiringIcons)) wiring[kind] = use(name);
  const sorted = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
  const data = { pages: sorted(byPage), branches: sorted(byBranch), wiring: sorted(wiring), icons: sorted(shapes) };
  return [
    '// Generated by crates/wirescript/scripts/gen_learn_tree.mjs from',
    '// crates/wasm/playground/learn.js: run `just learn-tree` after changing it.',
    `// ${icons().license}`,
    `window.wsLearnIcons = ${JSON.stringify(data, null, 1)};`,
    '',
  ].join('\n');
}

/**
 * What is wrong with a page's ```wiring diagrams (see docs/assets/ws-wiring.js):
 * a line that is not `<node> <port> -> <node> <port>` or `<node> "Name"`, or a
 * node whose kind has no icon in `kinds`.
 */
export function wiringProblems(text, kinds) {
  const problems = [];
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].trim() !== '```wiring') continue;
    for (i++; i < lines.length && lines[i].trim() !== '```'; i++) {
      const line = lines[i].trim();
      if (!line || line.startsWith('//')) continue;
      const tokens = [...line.matchAll(/"([^"]*)"|(\S+)/g)].map((m) => (m[1] !== undefined ? m[1] : m[2]));
      const named = tokens.length === 2 && line.endsWith('"');
      if (!named && (tokens.length !== 5 || tokens[2] !== '->')) {
        problems.push(`line ${i + 1}: not \`<node> <port> -> <node> <port>\` or \`<node> "Name"\`: ${line}`);
        continue;
      }
      for (const name of named ? [tokens[0]] : [tokens[0], tokens[3]]) {
        const kind = (name.match(/^([a-z]+?)\d*$/) || [])[1];
        if (!kind || !kinds[kind]) problems.push(`line ${i + 1}: no wiring kind "${name}" (add it to WIRING_ICONS)`);
      }
    }
  }
  return problems;
}

/** `text` with the block between the markers replaced by `tree`. */
export function withTree(text, tree, open = OPEN, close = CLOSE) {
  const eol = text.includes('\r\n') ? '\r\n' : '\n';
  const start = text.indexOf(open);
  const end = text.indexOf(close);
  if (start === -1 || end === -1 || end < start) {
    throw new Error(`the page needs ${open} and ${close} markers`);
  }
  return text.slice(0, start) + [open, tree.split('\n').join(eol), close].join(eol) + text.slice(end + close.length);
}

async function main() {
  const { LEARN, BRANCH_ICONS, WIRING_ICONS } = await import(pathToFileURL(join(repoRoot, 'crates', 'wasm', 'playground', 'learn.js')).href);
  const path = join(repoRoot, 'docs', 'src', 'learn', 'README.md');
  const next = withTree(readFileSync(path, 'utf8'), renderLearn(LEARN));
  const check = process.argv.includes('--check');
  if (!writeOrCheck(path, next, { check, what: 'gen_learn_tree: docs/src/learn/README.md', fix: 'just learn-tree' })) {
    process.exit(1);
  }
  const parts = LEARN.filter((p) => p.part && p.file !== CATALOG);
  if (parts.length) {
    const catalog = join(repoRoot, 'docs', 'src', CATALOG.replace(/^docs\//, ''));
    const text = withTree(readFileSync(catalog, 'utf8'), renderCatalog(LEARN), PARTS_OPEN, PARTS_CLOSE);
    if (!writeOrCheck(catalog, text, { check, what: `gen_learn_tree: ${CATALOG}`, fix: 'just learn-tree' })) {
      process.exit(1);
    }
  }
  const iconsFile = join(repoRoot, 'docs', 'assets', 'learn-icons.js');
  if (!writeOrCheck(iconsFile, renderBookIcons(LEARN, BRANCH_ICONS, WIRING_ICONS), { check, what: 'gen_learn_tree: docs/assets/learn-icons.js', fix: 'just learn-tree' })) {
    process.exit(1);
  }
  let stale = 0;
  for (const page of LEARN) {
    if (page.file === CATALOG) continue;
    const file = join(repoRoot, 'docs', 'src', page.file.replace(/^docs\//, ''));
    const text = withLinks(readFileSync(file, 'utf8'), renderLinks(page, LEARN));
    for (const p of wiringProblems(text, WIRING_ICONS)) {
      console.error(`gen_learn_tree: ${page.file}: ${p}`);
      stale++;
    }
    if (!writeOrCheck(file, text, { check, what: `gen_learn_tree: ${page.file}'s links`, fix: 'just learn-tree' })) stale++;
  }
  if (stale) process.exit(1);
  const lessons = LEARN.length - LEARN.filter((p) => p.part).length;
  console.log(
    `gen_learn_tree: ${check ? 'up to date' : 'wrote docs/src/learn/README.md'} (${lessons} lessons, ${parts.length} parts)`,
  );
}

if (isMain(import.meta.url)) await main();
