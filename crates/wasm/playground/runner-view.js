// The runner's page side, shared by the playground panel and the VS Code
// webview: each display drawn as pixels on a canvas, each seat a button laid
// out around the chip as placed, and the local player's keys sent as world
// script lines while they sit. It talks to the simulation only through
// `send`, and is fed the `layout`, `update` and `error` messages of
// runner-core.js, so it does not know which host it runs in.

import { KeyState, PLAYER, isRunnerKey } from './runner-core.js';

/** The color a transparent pixel shows: the panel behind it. */
const PANEL = [10, 10, 14];

/** The most chat lines the log keeps. */
const CHAT_LINES = 200;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

/**
 * Seats of one side in the order they appear on the page, which is how they
 * sit seen from above: the bottom row left to right, the top row right to
 * left (it faces the bottom one), the left column top to bottom and the
 * right column bottom to top.
 */
export function seatsOnPage(seats, side) {
  const row = seats.filter((s) => s.side === side);
  return side === 'top' || side === 'right' ? row.reverse() : row;
}

/**
 * Write `pixels` (`0xRRGGBB`, `-1` transparent) into RGBA `data`, starting
 * at pixel `at`.
 */
export function paint(data, pixels, at = 0) {
  pixels.forEach((p, i) => {
    const [r, g, b] = p < 0 ? PANEL : [(p >> 16) & 255, (p >> 8) & 255, p & 255];
    const o = (at + i) * 4;
    data[o] = r;
    data[o + 1] = g;
    data[o + 2] = b;
    data[o + 3] = 255;
  });
}

/** `lines` with `more` added at the end, keeping the last `max`. */
export function appendLines(lines, more, max = CHAT_LINES) {
  const all = lines.concat(more);
  return all.length > max ? all.slice(-max) : all;
}

/**
 * Build the runner in `root`. `send(msg)` delivers a message to the
 * simulation side. Returns `{ handle(msg), dispose() }`.
 */
export function createRunnerView(root, send) {
  const keys = new KeyState();
  let seated = null;
  const canvases = new Map();
  const seatButtons = new Map();

  root.replaceChildren();
  root.classList.add('ws-runner');
  const board = el('div', 'ws-runner-board');
  board.tabIndex = 0;
  const status = el('div', 'ws-runner-status', 'compiling...');
  const chat = el('pre', 'ws-runner-chat');
  let chatLines = [];
  root.append(board, status, chat);

  const sendLines = (lines) => lines.forEach((line) => send({ type: 'line', line }));
  const stand = () => {
    sendLines(keys.release());
    if (seated) send({ type: 'line', line: `stand ${PLAYER}` });
  };

  const onKey = (down) => (e) => {
    if (!seated || !isRunnerKey(e.code)) return;
    e.preventDefault();
    if (e.code === 'Escape') {
      if (down) stand();
      return;
    }
    if (!e.repeat) sendLines(keys.key(e.code, down));
  };
  const onMouse = (down) => (e) => {
    if (!seated || e.target.closest('.ws-runner-seat')) return;
    e.preventDefault();
    sendLines(keys.mouse(e.button, down));
  };
  const keydown = onKey(true);
  const keyup = onKey(false);
  const mousedown = onMouse(true);
  const mouseup = onMouse(false);
  const blur = () => sendLines(keys.release());
  const noMenu = (e) => seated && e.preventDefault();
  board.addEventListener('keydown', keydown);
  board.addEventListener('keyup', keyup);
  board.addEventListener('mousedown', mousedown);
  board.addEventListener('mouseup', mouseup);
  board.addEventListener('contextmenu', noMenu);
  board.addEventListener('blur', blur);

  function layout(l) {
    board.replaceChildren();
    canvases.clear();
    seatButtons.clear();
    const middle = el('div', 'ws-runner-middle');
    const screens = el('div', 'ws-runner-displays');
    for (const d of l.displays) {
      const fig = el('figure', 'ws-runner-display');
      const canvas = el('canvas');
      canvas.width = d.width;
      canvas.height = d.height;
      canvas.dataset.display = d.name;
      // Whole-pixel scaling keeps every pixel square on any display size.
      const scale = Math.max(1, Math.floor(320 / Math.max(d.width, d.height)));
      canvas.style.width = `${d.width * scale}px`;
      canvas.style.height = `${d.height * scale}px`;
      const ctx = canvas.getContext('2d');
      const image = ctx.createImageData(d.width, d.height);
      paint(image.data, new Array(d.width * d.height).fill(-1));
      ctx.putImageData(image, 0, 0);
      canvases.set(d.name, { ctx, image, width: d.width });
      fig.append(canvas, el('figcaption', null, d.name));
      screens.append(fig);
    }
    middle.append(screens, el('div', 'ws-runner-chip', 'chip'));
    const side = (name) => {
      const box = el('div', `ws-runner-side ws-runner-${name}`);
      for (const s of seatsOnPage(l.seats, name)) {
        const b = el('button', 'ws-runner-seat', s.label);
        b.type = 'button';
        b.dataset.seat = s.name;
        b.title = `Sit in ${s.name}`;
        if (s.color) b.style.setProperty('--seat-color', s.color);
        b.addEventListener('click', () => {
          if (seated !== s.name) {
            sendLines(keys.release());
            send({ type: 'line', line: `sit ${PLAYER} ${s.name}` });
          }
          board.focus();
        });
        seatButtons.set(s.name, b);
        box.append(b);
      }
      return box;
    };
    board.append(side('top'), side('left'), middle, side('right'), side('bottom'));
    if (!l.displays.length && !l.seats.length) {
      status.textContent = 'This program has no @display or @seat.';
    }
  }

  function update(u) {
    for (const p of u.displays) {
      const c = canvases.get(p.name);
      if (!c) continue;
      for (const row of p.rows) paint(c.image.data, row.pixels, row.y * c.width);
      c.ctx.putImageData(c.image, 0, 0);
    }
    seated = Object.entries(u.seated).find(([, who]) => who === PLAYER)?.[0] ?? null;
    for (const [name, b] of seatButtons) {
      const who = u.seated[name];
      b.classList.toggle('taken', !!who);
      b.classList.toggle('mine', who === PLAYER);
    }
    if (u.chat.length) {
      chatLines = appendLines(chatLines, u.chat.map((l) => l.text));
      chat.textContent = chatLines.join('\n');
      chat.scrollTop = chat.scrollHeight;
    }
    const where = seated ? `sitting in ${seated}: W A S D, Space, E Q C, mouse; Esc stands up` : 'click a seat to sit';
    status.textContent = u.error ? `stopped at tick ${u.tick}: ${u.error}` : `tick ${u.tick} - ${where}`;
  }

  return {
    handle(msg) {
      if (msg.type === 'layout') layout(msg.layout);
      else if (msg.type === 'update') update(msg.update);
      else if (msg.type === 'error') status.textContent = msg.error;
    },
    dispose() {
      board.removeEventListener('keydown', keydown);
      board.removeEventListener('keyup', keyup);
      board.removeEventListener('mousedown', mousedown);
      board.removeEventListener('mouseup', mouseup);
      board.removeEventListener('contextmenu', noMenu);
      board.removeEventListener('blur', blur);
      root.replaceChildren();
    },
  };
}
