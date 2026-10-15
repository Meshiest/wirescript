import { test } from 'node:test';
import assert from 'node:assert/strict';
import { layoutGraph, renderCatalog, renderGraph, renderBookIcons, renderLearn, renderLinks, renderTree, wiringProblems, withLinks, withTree } from '../gen_learn_tree.mjs';

const pages = [
  { id: 'T0', title: 'First', file: 'docs/learn/first.md', branch: 'Trunk', requires: [] },
  { id: 'T1', title: 'Second', file: 'docs/learn/second.md', branch: 'Trunk', requires: ['T0'] },
  { id: 'C1', title: 'ALU', file: 'docs/learn/cpu/alu.md', branch: 'CPU', requires: ['T1'] },
  { id: 'C2', title: 'Clock', file: 'docs/learn/cpu/clock.md', branch: 'CPU', requires: ['C1', 'T0'] },
  { id: 'A1', title: 'Weather', file: 'docs/learn/weather.md', branch: 'Admin', requires: [], admin: true },
];

test('one numbered list per branch, noting only requirements from elsewhere', () => {
  const tree = renderTree(pages);
  assert.match(tree, /### Trunk\n\n1\. \[First\]\(first\.md\)\n2\. \[Second\]\(second\.md\)\n/);
  assert.match(tree, /1\. \[ALU\]\(cpu\/alu\.md\) \(after \[Second\]\(second\.md\)\)/);
  assert.match(tree, /2\. \[Clock\]\(cpu\/clock\.md\) \(after \[First\]\(first\.md\)\)/);
  assert.match(tree, /\[Weather\]\(weather\.md\) \(needs admin to load\)/);
  assert.ok(tree.startsWith('<div class="learn-tree">\n\n'));
});

test('an unknown requirement is an error', () => {
  assert.throws(() => renderTree([{ id: 'X', title: 'X', file: 'docs/learn/x.md', branch: 'B', requires: ['nope'] }]), /unknown node nope/);
});

test('replaces only the marked block and keeps the line endings', () => {
  const text = 'intro\r\n<!-- learn-tree -->\r\nold\r\n<!-- /learn-tree -->\r\nend\r\n';
  assert.equal(withTree(text, 'a\nb'), 'intro\r\n<!-- learn-tree -->\r\na\r\nb\r\n<!-- /learn-tree -->\r\nend\r\n');
  assert.throws(() => withTree('no markers', 'x'), /markers/);
});

test('titles are escaped and long ones wrap; requirements must come first', () => {
  const svg = renderGraph([{ id: 'X', title: 'A < B & "C"', file: 'docs/learn/x.md', branch: 'B', requires: [] }]);
  assert.match(svg, />A &lt; B &amp; &quot;C&quot;<\/tspan>/);
  const long = renderGraph([{ id: 'X', title: 'Templates Chosen at Run Time', file: 'docs/learn/x.md', branch: 'B', requires: [] }]);
  assert.equal((long.match(/<tspan /g) || []).length, 2);
  assert.throws(
    () => layoutGraph([
      { id: 'A', title: 'A', file: 'docs/learn/a.md', branch: 'B', requires: ['B'] },
      { id: 'B', title: 'B', file: 'docs/learn/b.md', branch: 'B', requires: [] },
    ]),
    /must come after its requirement B/,
  );
});

test('the page block holds the graph, then the list', () => {
  const block = renderLearn(pages);
  assert.ok(block.indexOf('<svg') < block.indexOf('### Trunk'));
  assert.ok(block.startsWith('<div class="learn-tree">') && block.endsWith('</div>'));
});

test('parts stay out of the graph and list, and fill the catalog', () => {
  const withParts = [
    ...pages,
    { id: 'Parts', title: 'Parts', file: 'docs/learn/parts/README.md', branch: 'Parts', requires: [], part: true },
    { id: 'P1', title: 'Lamp', file: 'docs/learn/parts/lamp.md', branch: 'Building Parts', requires: ['T1'], part: true, summary: 'Switch, Point Light' },
  ];
  const block = renderLearn(withParts);
  assert.ok(!block.includes('Lamp') && !block.includes('Building Parts'));
  const catalog = renderCatalog(withParts);
  assert.match(catalog, /### Building Parts\n\n\| Part \| Uses \| Builds on \|\n\|---\|---\|---\|\n\| \[Lamp\]\(lamp\.md\) \| Switch, Point Light \| \[Second\]\(\.\.\/second\.md\) \|/);
  assert.ok(!catalog.includes('[Parts]'));
});

test('a graph: false page is listed but not drawn', () => {
  const block = renderLearn([...pages, { id: 'T9', title: 'Fast Path', file: 'docs/learn/fast.md', branch: 'Trunk', requires: ['T0'], graph: false }]);
  assert.ok(block.includes('[Fast Path](fast.md)'));
  assert.ok(!block.includes('href="fast.html"'));
});

test('a branch hangs below its entry in the same band, in the column right of it', () => {
  const { branches, nodes } = layoutGraph(pages);
  const [trunk, cpu, admin] = branches;
  assert.deepEqual([trunk.slot, cpu.slot, admin.slot], [0, 1, 2]);
  assert.equal(cpu.entry.page.id, 'T1');
  assert.ok(nodes.get('C1').top > nodes.get('T1').top);
  assert.equal(nodes.get('C2').top - nodes.get('C1').top, nodes.get('T1').top - nodes.get('T0').top);
  assert.equal(admin.entry, null);
});

test('branches hanging from one column sit right after it, deepest entry first', () => {
  const { branches } = layoutGraph([
    { id: 'A', title: 'A', file: 'docs/learn/a.md', branch: 'Base', requires: [] },
    { id: 'B', title: 'B', file: 'docs/learn/b.md', branch: 'Base', requires: ['A'] },
    { id: 'S', title: 'S', file: 'docs/learn/s.md', branch: 'Shallow', requires: ['A'] },
    { id: 'D', title: 'D', file: 'docs/learn/d.md', branch: 'Deep', requires: ['B'] },
    { id: 'R', title: 'R', file: 'docs/learn/r.md', branch: 'Root', requires: [] },
  ]);
  assert.deepEqual(branches.map((b) => [b.name, b.slot]), [['Base', 0], ['Shallow', 2], ['Deep', 1], ['Root', 3]]);
});

test('later bands connect to their entries, one lane per entry across the strip', () => {
  const many = [
    { id: 'B0', title: 'Base', file: 'docs/learn/base.md', branch: 'Base', requires: [] },
    { id: 'B1', title: 'Base 2', file: 'docs/learn/base2.md', branch: 'Base', requires: ['B0'] },
  ];
  for (let i = 1; i <= 8; i++) {
    many.push({ id: `X${i}`, title: `X${i}`, file: `docs/learn/x${i}.md`, branch: `Branch ${i}`, requires: [i > 6 ? 'B1' : 'B0'] });
  }
  const { bands, perBand, branches } = layoutGraph(many, 7);
  assert.deepEqual([bands, perBand], [2, 5]);
  const later = branches.filter((b) => b.band === 1);
  assert.equal(later.length, 4);
  assert.ok(later.every((b) => b.route && b.entry));
  // Two entries (B0, B1) cross into band 1, on two different lanes.
  const across = (b) => b.route.split(' ')[3].split(',')[1];
  assert.equal(new Set(later.map(across)).size, 2);
  const svg = renderGraph(many);
  // Every branch but Base has a line into it; Base has one line down itself.
  assert.equal((svg.match(/class="edge"/g) || []).length, 8 + 1);
});

test('a lesson is an icon square with its title, peppers and marks', () => {
  const svg = renderGraph([
    ...pages.slice(0, 2),
    { id: 'H', title: 'Hard', file: 'docs/learn/hard.md', branch: 'Trunk', requires: ['T1'], icon: 'dice', level: 3 },
    { id: 'O', title: 'Other', file: 'docs/learn/other.md', branch: 'Side', requires: ['T0'] },
    { id: 'O2', title: 'Other Two', file: 'docs/learn/other2.md', branch: 'Side', requires: ['O', 'H'] },
  ]);
  assert.match(svg, /<a class="learn-node" href="hard\.html" style="--c: hsl\([^)]*\)"><title>Hard - difficulty 3 of 3 - after Second<\/title><rect class="sq"/);
  const hard = svg.slice(svg.indexOf('href="hard.html"'), svg.indexOf('</a>', svg.indexOf('href="hard.html"')));
  assert.equal((hard.match(/class="pepper"/g) || []).length, 3);
  // A lesson with no level shows no peppers and no difficulty.
  const first = svg.slice(svg.indexOf('href="first.html"'), svg.indexOf('</a>', svg.indexOf('href="first.html"')));
  assert.ok(!first.includes('class="pepper"') && !first.includes('difficulty'));
  assert.match(hard, /class="done"/);
  assert.match(hard, />Hard<\/tspan>/);
  // Other Two builds on Hard from another branch: a corner badge.
  const two = svg.slice(svg.indexOf('href="other2.html"'), svg.indexOf('</a>', svg.indexOf('href="other2.html"')));
  assert.match(two, /class="badge"/);
  assert.match(svg, /text-decoration: none/);
  assert.ok(!/\n\s*\n/.test(svg));
});

test('a band starting from nothing sits in the middle, its branches alternating right and left', () => {
  const pagesCentered = [
    { id: 'A', title: 'A', file: 'docs/learn/a.md', branch: 'Base', requires: [] },
    { id: 'B', title: 'B', file: 'docs/learn/b.md', branch: 'Base', requires: ['A'] },
    { id: 'C', title: 'C', file: 'docs/learn/c.md', branch: 'Base', requires: ['B'] },
    { id: 'X', title: 'X', file: 'docs/learn/x.md', branch: 'Deepest', requires: ['C'] },
    { id: 'Y', title: 'Y', file: 'docs/learn/y.md', branch: 'Middle', requires: ['B'] },
    { id: 'Z', title: 'Z', file: 'docs/learn/z.md', branch: 'Shallow', requires: ['A'] },
  ];
  const { branches, width } = layoutGraph(pagesCentered);
  assert.equal(width, 5);
  // Base in the middle; Deepest right, Middle left, Shallow right of Deepest.
  assert.deepEqual(branches.map((b) => [b.name, b.slot]), [['Base', 2], ['Deepest', 3], ['Middle', 1], ['Shallow', 4]]);
});

test('the first lesson of a branch shows no corner badge for the entry its line already shows', () => {
  const svg = renderGraph(pages);
  const alu = svg.slice(svg.indexOf('href="cpu/alu.html"'), svg.indexOf('</a>', svg.indexOf('href="cpu/alu.html"')));
  assert.ok(!alu.includes('class="badge"'));
});

test('a page links what it builds on, the lessons after it, and the parts that use it', () => {
  const all = [
    ...pages,
    { id: 'P1', title: 'Lamp', file: 'docs/learn/parts/lamp.md', branch: 'Parts', requires: ['T1'], part: true },
  ];
  const block = renderLinks(all.find((p) => p.id === 'T1'), all);
  assert.match(block, /^<div class="learn-links">/);
  assert.match(block, /\*\*Before this:\*\* \[First\]\(first\.md\)/);
  assert.match(block, /\*\*Next steps:\*\* \[ALU\]\(cpu\/alu\.md\)/);
  assert.match(block, /\*\*Parts that use this:\*\* \[Lamp\]\(parts\/lamp\.md\)/);
  assert.match(renderLinks(all.find((p) => p.id === 'C2'), all), /\[ALU\]\(alu\.md\), \[First\]\(\.\.\/first\.md\)/);
  assert.equal(renderLinks(all.find((p) => p.id === 'A1'), all), '');
});

test('the links block goes under the title, is replaced in place, and an empty one is removed', () => {
  const page = '# Title\r\n\r\nIntro.\r\n';
  const once = withLinks(page, 'one');
  assert.equal(once, '# Title\r\n\r\n<!-- learn-links -->\r\none\r\n<!-- /learn-links -->\r\n\r\nIntro.\r\n');
  assert.equal(withLinks(once, 'two'), once.replace('one', 'two'));
  assert.equal(withLinks(once, ''), page);
  assert.throws(() => withLinks('no title', 'x'), /title/);
});

test('the book icons name each page by its book path, each branch by name, and each wiring kind', () => {
  const text = renderBookIcons(
    [
      { id: 'A', title: 'A', file: 'docs/learn/random/dice.md', branch: 'Random', requires: [], icon: 'dice' },
      { id: 'P', title: 'Parts', file: 'docs/learn/parts/README.md', requires: [], part: true, icon: 'toolbox' },
      { id: 'N', title: 'No icon', file: 'docs/learn/none.md', branch: 'Random', requires: [] },
    ],
    { Random: 'dice-d20' },
    { button: 'hand-pointer' },
  );
  const data = JSON.parse(text.slice(text.indexOf('{'), text.lastIndexOf('}') + 1));
  assert.deepEqual(data.pages, { 'learn/parts/index': 'toolbox', 'learn/random/dice': 'dice' });
  assert.deepEqual(data.branches, { Random: 'dice-d20' });
  assert.deepEqual(data.wiring, { button: 'hand-pointer' });
  assert.deepEqual(Object.keys(data.icons), ['dice', 'dice-d20', 'hand-pointer', 'toolbox']);
  assert.throws(() => renderBookIcons([], { X: 'no-such-icon' }), /fetch_fa_icons/);
});

test('a wiring diagram line names two nodes of known kinds and their ports', () => {
  const kinds = { switch: 'toggle-on', chip: 'microchip', light: 'lightbulb' };
  const page = ['# T', '', '```wiring', 'chip "Counter"', 'switch1 Enabled -> chip a', 'chip b -> light "Is Enabled"', '```', ''].join('\n');
  assert.deepEqual(wiringProblems(page, kinds), []);
  const bad = ['```wiring', 'switch Enabled chip a', 'lamp X -> chip b', '```'].join('\n');
  const problems = wiringProblems(bad, kinds);
  assert.equal(problems.length, 2);
  assert.match(problems[0], /not `<node> <port> -> <node> <port>`/);
  assert.match(problems[1], /no wiring kind "lamp"/);
});
