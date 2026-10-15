// ws-wiring.js -- draw the Learn pages' ```wiring blocks: how to wire a
// compiled chip in-game, as columns of icons with a line per wire and the
// port names at the two ends of each line.
//
//   ```wiring
//   switch Enabled -> chip a
//   chip b -> light Enabled
//   ```
//
// Each line is one wire: `<from> <port> -> <to> <port>`, a port with spaces in
// quotes; `<node> "Name"` puts a name above a node (used when a page has two
// chips). A node is a kind (`button`, `light`, ...; see WIRING_ICONS in
// playground/learn.js) with an optional number when there are several
// (`button1`, `button2`); `chip` is the pasted chip, `chip2` another one. The
// icons come from learn-icons.js. Where a block cannot be drawn, its text
// stays.
(function () {
  'use strict';

  const SVG = 'http://www.w3.org/2000/svg';
  const ROW = 46; // vertical room per port: one component box and the gap below it
  const NODE = 36; // a component's box
  const ICON = 20;
  const CHIP = 76; // the pasted chip's box
  const CHIP_ICON = 42;
  const GAP_Y = 10;
  const NAME_ROOM = 22; // above a named node
  const MIN_GAP_X = 56;
  const LABEL_PAD = 7;
  const FONT = 12;

  /** The wires and node names of a block, or throws on a line it cannot read. */
  function parse(text) {
    const wires = [];
    const names = new Map();
    for (const raw of text.split('\n')) {
      const line = raw.trim();
      if (!line || line.startsWith('//')) continue;
      const tokens = [];
      for (const m of line.matchAll(/"([^"]*)"|(\S+)/g)) tokens.push(m[1] !== undefined ? m[1] : m[2]);
      if (tokens.length === 2 && line.endsWith('"')) {
        names.set(tokens[0], tokens[1]);
        continue;
      }
      if (tokens.length !== 5 || tokens[2] !== '->') throw new Error('cannot read wire: ' + line);
      wires.push({ from: tokens[0], fromPort: tokens[1], to: tokens[3], toPort: tokens[4] });
    }
    if (!wires.length) throw new Error('no wires');
    return { wires, names };
  }

  function nodeInfo(name) {
    const m = name.match(/^([a-z]+?)(\d*)$/);
    if (!m) throw new Error('bad node name: ' + name);
    return { kind: m[1], num: m[2] };
  }

  function measureWith(font) {
    const ctx = document.createElement('canvas').getContext('2d');
    ctx.font = FONT + 'px ' + font;
    return (s) => ctx.measureText(s).width;
  }

  /** Columns, rows and coordinates for every node, port and wire. */
  function layout(wires, measure, names = new Map()) {
    const nodes = new Map();
    const node = (name) => {
      if (!nodes.has(name)) {
        const info = nodeInfo(name);
        const main = name === 'chip';
        const chipLike = info.kind === 'chip';
        nodes.set(name, {
          name,
          ...info,
          main,
          chipLike,
          ins: [],
          outs: [],
          width: main ? CHIP : NODE,
          minHeight: main ? CHIP : NODE,
          dummy: false,
        });
      }
      return nodes.get(name);
    };
    const add = (list, p) => {
      if (!list.includes(p)) list.push(p);
    };
    for (const w of wires) {
      add(node(w.from).outs, w.fromPort);
      add(node(w.to).ins, w.toPort);
    }

    // Columns: each node one right of the furthest node wired into it.
    const col = new Map([...nodes.keys()].map((n) => [n, 0]));
    for (let pass = 0; pass < nodes.size; pass++) {
      let moved = false;
      for (const w of wires) {
        if (col.get(w.to) <= col.get(w.from)) {
          col.set(w.to, col.get(w.from) + 1);
          moved = true;
        }
      }
      if (!moved) break;
    }
    const columns = [];
    for (const [name, c] of col) (columns[c] = columns[c] || []).push(nodes.get(name));

    // A wire that skips columns passes through a point in each one between.
    const segments = [];
    let dummies = 0;
    for (const w of wires) {
      const a = col.get(w.from);
      const b = col.get(w.to);
      let prev = { node: nodes.get(w.from), port: w.fromPort, side: 'out' };
      for (let c = a + 1; c < b; c++) {
        const d = { name: '~' + dummies++, dummy: true, ins: ['~'], outs: ['~'], width: 0, minHeight: NODE };
        columns[c].push(d);
        segments.push({ a: prev, b: { node: d, port: '~', side: 'in' }, wire: w });
        prev = { node: d, port: '~', side: 'out' };
      }
      segments.push({ a: prev, b: { node: nodes.get(w.to), port: w.toPort, side: 'in' }, wire: w });
    }

    // Ports sit ROW apart, centered on their side, so a column of one-port
    // boxes lines up with the pins of a node beside it.
    for (const n of columns.flat()) {
      n.height = Math.max(n.minHeight, (Math.max(n.ins.length, n.outs.length, 1) - 1) * ROW + NODE);
      // Room for a name above the node.
      n.above = names.has(n.name) ? NAME_ROOM : 0;
    }
    const portY = (end) => {
      const list = end.side === 'in' ? end.node.ins : end.node.outs;
      const i = Math.max(0, list.indexOf(end.port));
      return end.node.y + end.node.height / 2 + (i - (list.length - 1) / 2) * ROW;
    };
    const place = () => {
      for (const column of columns) {
        let y = 0;
        for (const n of column) {
          n.y = y + (n.above || 0);
          y = n.y + n.height + GAP_Y;
        }
      }
    };

    // Order each column by where its wires come from, then the other way.
    place();
    const linked = (n, side) =>
      segments.filter((s) => (side === 'in' ? s.b.node === n : s.a.node === n)).map((s) => (side === 'in' ? s.a : s.b));
    const center = (n, side) => {
      const ends = linked(n, side);
      return ends.length ? ends.reduce((t, e) => t + portY(e), 0) / ends.length : n.y + n.height / 2;
    };
    // Ports in the order of what they wire to.
    const sortPorts = () => {
      for (const n of nodes.values()) {
        const at = (side, port) => {
          const ends = segments
            .filter((s) => (side === 'in' ? s.b.node === n && s.b.port === port : s.a.node === n && s.a.port === port))
            .map((s) => (side === 'in' ? s.a : s.b));
          return ends.length ? ends.reduce((t, e) => t + portY(e), 0) / ends.length : 0;
        };
        n.ins.sort((p, q) => at('in', p) - at('in', q));
        n.outs.sort((p, q) => at('out', p) - at('out', q));
      }
    };
    for (let sweep = 0; sweep < 4; sweep++) {
      const forward = sweep % 2 === 0;
      const order = forward ? columns.slice(1) : columns.slice(0, -1).reverse();
      for (const column of order) {
        const keys = new Map(column.map((n) => [n, center(n, forward ? 'in' : 'out')]));
        column.sort((p, q) => keys.get(p) - keys.get(q));
        place();
      }
      sortPorts();
    }

    // Swap neighbors in a column while that removes crossing wires.
    const crossings = () => {
      let n = 0;
      for (let i = 0; i < segments.length; i++) {
        for (let j = i + 1; j < segments.length; j++) {
          const s = segments[i];
          const t = segments[j];
          if (columnOf.get(s.a.node) !== columnOf.get(t.a.node)) continue;
          if ((portY(s.a) - portY(t.a)) * (portY(s.b) - portY(t.b)) < 0) n++;
        }
      }
      return n;
    };
    const columnOf = new Map();
    columns.forEach((column, c) => column.forEach((n) => columnOf.set(n, c)));
    let best = crossings();
    for (let pass = 0; pass < 8 && best > 0; pass++) {
      let improved = false;
      for (const column of columns) {
        for (let i = 0; i + 1 < column.length; i++) {
          const ports = [...nodes.values()].map((n) => [n.ins.slice(), n.outs.slice()]);
          [column[i], column[i + 1]] = [column[i + 1], column[i]];
          place();
          sortPorts();
          const now = crossings();
          if (now < best) {
            best = now;
            improved = true;
          } else {
            [column[i], column[i + 1]] = [column[i + 1], column[i]];
            [...nodes.values()].forEach((n, k) => {
              n.ins = ports[k][0];
              n.outs = ports[k][1];
            });
            place();
          }
        }
      }
      if (!improved) break;
    }

    // Line nodes up with what they wire to, keeping the column's order.
    for (let sweep = 0; sweep < 6; sweep++) {
      const forward = sweep % 2 === 0;
      for (const column of forward ? columns : columns.slice().reverse()) {
        let floor = -Infinity;
        for (const n of column) {
          const ends = linked(n, 'in').concat(linked(n, 'out'));
          const want = ends.length ? ends.reduce((t, e) => t + portY(e), 0) / ends.length - n.height / 2 : n.y;
          n.y = Math.max(want, floor + (n.above || 0));
          floor = n.y + n.height + GAP_Y;
        }
      }
    }
    const top = Math.min(...columns.flat().map((n) => n.y));
    for (const n of columns.flat()) n.y -= top;

    // Columns as far apart as the port names at the ends of their wires need.
    let x = 0;
    columns.forEach((column, c) => {
      const width = Math.max(...column.map((n) => n.width));
      for (const n of column) n.x = x + (width - n.width) / 2;
      if (c === columns.length - 1) return;
      let need = MIN_GAP_X;
      for (const s of segments) {
        if (!column.includes(s.a.node)) continue;
        const left = s.a.node.dummy ? 0 : measure(s.a.port) + LABEL_PAD;
        const right = s.b.node.dummy ? 0 : measure(s.b.port) + LABEL_PAD;
        need = Math.max(need, left + right + 24);
      }
      x += width + need;
    });

    const all = columns.flat();
    return {
      nodes: all.filter((n) => !n.dummy),
      segments: segments.map((s) => ({
        x1: s.a.node.x + s.a.node.width,
        y1: portY(s.a),
        x2: s.b.node.x,
        y2: portY(s.b),
        from: s.a.node.dummy ? null : s.a.port,
        to: s.b.node.dummy ? null : s.b.port,
        fromKey: s.a.node.name + ' out ' + s.a.port,
        toKey: s.b.node.name + ' in ' + s.b.port,
        start: !s.a.node.dummy,
        end: !s.b.node.dummy,
      })),
      width: Math.max(...all.map((n) => n.x + n.width)),
      height: Math.max(...all.map((n) => n.y + n.height)),
    };
  }

  function el(name, attrs, parent) {
    const e = document.createElementNS(SVG, name);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
    if (parent) parent.appendChild(e);
    return e;
  }

  function icon(parent, kind, cx, cy, size, cls) {
    const data = window.wsLearnIcons;
    const name = data && data.wiring && data.wiring[kind];
    const shape = name && data.icons[name];
    if (!shape) throw new Error('no icon for "' + kind + '"');
    const scale = size / Math.max(shape[0], shape[1]);
    const g = el('g', { class: cls, transform: `translate(${cx - (shape[0] * scale) / 2} ${cy - (shape[1] * scale) / 2}) scale(${scale})` }, parent);
    el('path', { d: shape[2] }, g);
  }

  const title = (n) => {
    const words = { decal: 'Text Decal', changer: 'Brick Property Changer', circuit: 'Other circuit', chip: 'Chip' };
    const base = words[n.kind] || n.kind.charAt(0).toUpperCase() + n.kind.slice(1);
    return n.main ? 'The chip' : base + (n.num ? ' ' + n.num : '');
  };

  function render({ wires, names }, font) {
    const measure = measureWith(font);
    const g = layout(wires, measure, names);
    const PAD = 10;
    // Room for the names above their nodes.
    let left = -PAD;
    let right = g.width + PAD;
    let top = -PAD - 6;
    for (const n of g.nodes) {
      if (!names.has(n.name)) continue;
      const half = measure(names.get(n.name)) / 2 + 2;
      left = Math.min(left, n.x + n.width / 2 - half - PAD);
      right = Math.max(right, n.x + n.width / 2 + half + PAD);
      top = Math.min(top, n.y - 24 - PAD);
    }
    const bottom = g.height + PAD;
    const svg = el('svg', {
      class: 'ws-wiring-svg',
      viewBox: `${left} ${top} ${right - left} ${bottom - top}`,
      width: right - left,
      role: 'img',
    });
    el('title', {}, svg).textContent = wires.map((w) => `${w.from} ${w.fromPort} to ${w.to} ${w.toPort}`).join('; ');

    const lines = el('g', { class: 'ws-wiring-lines' }, svg);
    const ends = el('g', { class: 'ws-wiring-ends' }, svg);
    for (const s of g.segments) {
      const dx = Math.max(24, (s.x2 - s.x1) / 2);
      el('path', { d: `M${s.x1} ${s.y1} C${s.x1 + dx} ${s.y1} ${s.x2 - dx} ${s.y2} ${s.x2} ${s.y2}` }, lines);
      if (s.start) el('circle', { cx: s.x1, cy: s.y1, r: 2.5 }, ends);
      if (s.end) el('circle', { cx: s.x2, cy: s.y2, r: 2.5 }, ends);
    }

    for (const n of g.nodes) {
      const box = el('g', { class: 'ws-wiring-node' + (n.chipLike ? ' ws-wiring-chip' : '') }, svg);
      el('title', {}, box).textContent = title(n);
      el('rect', { x: n.x, y: n.y, width: n.width, height: n.height, rx: 6 }, box);
      icon(box, n.kind, n.x + n.width / 2, n.y + n.height / 2, n.main ? CHIP_ICON : ICON, 'ws-wiring-icon');
      if (names.has(n.name)) {
        const t = el('text', { x: n.x + n.width / 2, y: n.y - 8, class: 'ws-wiring-name', 'text-anchor': 'middle' }, box);
        t.textContent = names.get(n.name);
      } else if (n.num) {
        const t = el('text', { x: n.x + n.width - 4, y: n.y + n.height - 4, class: 'ws-wiring-num', 'text-anchor': 'end' }, box);
        t.textContent = n.num;
      }
    }

    const labels = el('g', { class: 'ws-wiring-labels' }, svg);
    const named = new Set();
    for (const s of g.segments) {
      // A port with several wires is named once.
      if (s.from && !named.has(s.fromKey)) {
        named.add(s.fromKey);
        el('text', { x: s.x1 + LABEL_PAD, y: s.y1 - 5 }, labels).textContent = s.from;
      }
      if (s.to && !named.has(s.toKey)) {
        named.add(s.toKey);
        el('text', { x: s.x2 - LABEL_PAD, y: s.y2 - 5, 'text-anchor': 'end' }, labels).textContent = s.to;
      }
    }
    return svg;
  }

  function init() {
    const blocks = document.querySelectorAll('pre > code.language-wiring');
    if (!blocks.length) return;
    const font = getComputedStyle(document.body).fontFamily;
    for (const code of blocks) {
      const pre = code.parentElement;
      try {
        const figure = document.createElement('div');
        figure.className = 'ws-wiring';
        figure.appendChild(render(parse(code.textContent), font));
        pre.replaceWith(figure);
      } catch (e) {
        console.warn('ws-wiring: could not draw a diagram:', e);
      }
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
