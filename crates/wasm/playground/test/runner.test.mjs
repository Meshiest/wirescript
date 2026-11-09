import { test } from 'node:test';
import assert from 'node:assert/strict';
import { KeyState, isRunnerKey, serveRunner } from '../runner-core.js';
import { appendLines, paint, seatsOnPage } from '../runner-view.js';

test('W and S drive Forward, and holding both cancels out', () => {
  const k = new KeyState();
  assert.deepEqual(k.key('KeyW', true), ['input you Forward 1']);
  assert.deepEqual(k.key('KeyS', true), ['input you Forward 0']);
  assert.deepEqual(k.key('KeyW', false), ['input you Forward -1']);
  assert.deepEqual(k.key('KeyS', false), ['input you Forward 0']);
});

test('A and D drive Right, Space drives Up', () => {
  const k = new KeyState();
  assert.deepEqual(k.key('KeyA', true), ['input you Right -1']);
  assert.deepEqual(k.key('KeyD', true), ['input you Right 0']);
  assert.deepEqual(k.key('Space', true), ['input you Up 1']);
  assert.deepEqual(k.key('Space', false), ['input you Up 0']);
});

test('buttons and mouse buttons hold until released, once each', () => {
  const k = new KeyState();
  assert.deepEqual(k.key('KeyE', true), ['input you E 1']);
  assert.deepEqual(k.key('KeyE', true), [], 'a held key does not repeat');
  assert.deepEqual(k.key('KeyQ', true), ['input you Q 1']);
  assert.deepEqual(k.mouse(0, true), ['input you LeftMouse 1']);
  assert.deepEqual(k.mouse(2, true), ['input you RightMouse 1']);
  assert.deepEqual(k.mouse(1, true), [], 'the middle button is not an input');
  assert.deepEqual(k.key('KeyE', false), ['input you E 0']);
});

test('releasing everything zeroes what was held', () => {
  const k = new KeyState();
  k.key('KeyW', true);
  k.key('KeyC', true);
  k.mouse(0, true);
  assert.deepEqual(k.release().sort(), ['input you C 0', 'input you Forward 0', 'input you LeftMouse 0']);
  assert.deepEqual(k.release(), []);
});

test('only the runner keys are captured', () => {
  for (const code of ['KeyW', 'KeyA', 'KeyS', 'KeyD', 'Space', 'KeyE', 'KeyQ', 'KeyC', 'Escape']) {
    assert.ok(isRunnerKey(code), code);
  }
  for (const code of ['ArrowUp', 'KeyX', 'Enter', 'Tab']) assert.ok(!isRunnerKey(code), code);
});

// A stand-in for the wasm `Runner`: counts ticks, takes lines.
class FakeRunner {
  constructor(source) {
    if (source === 'bad') throw new Error('compile: bad');
    this.source = source;
    this.tick = -1;
    this.lines = [];
  }
  layout() {
    return JSON.stringify({ displays: [{ name: 's', width: 1, height: 1 }], seats: [] });
  }
  step(n) {
    this.tick += n;
    if (this.source === 'trap') throw new Error('unreachable');
    const chat = this.tick === 5 ? [{ tick: 5, text: 'hi' }] : [];
    return JSON.stringify({ tick: this.tick, chat, displays: [], seated: {}, error: null });
  }
  push(line) {
    this.lines.push(line);
    return line.startsWith('bogus') ? 'unknown action' : '';
  }
}

function harness() {
  const out = [];
  const port = { postMessage: (m) => out.push(m) };
  const timers = { setInterval: (f) => ((port.tick = f), 1), clearInterval: () => (port.tick = null) };
  serveRunner(port, FakeRunner, timers);
  return { port, out, send: (data) => port.onmessage({ data }) };
}

test('a start posts the layout and the first tick, then only ticks that change something', () => {
  const { port, out, send } = harness();
  send({ type: 'start', source: 'ok' });
  assert.equal(out[0].type, 'layout');
  assert.deepEqual(out[1], { type: 'update', update: { tick: 0, chat: [], displays: [], seated: {}, error: null } });
  for (let i = 0; i < 5; i++) port.tick();
  assert.equal(out.length, 3, 'quiet ticks 1 to 4 post nothing');
  assert.deepEqual(out[2].update.chat, [{ tick: 5, text: 'hi' }]);
  for (let i = 0; i < 30; i++) port.tick();
  assert.equal(out.length, 4, 'a quiet run still reports every 30 ticks');
  send({ type: 'stop' });
  assert.equal(port.tick, null, 'stop clears the loop');
});

test('a step that throws stops the loop and says so', () => {
  const { port, out, send } = harness();
  send({ type: 'start', source: 'trap' });
  assert.equal(out.at(-1).type, 'error');
  assert.match(out.at(-1).error, /the runner stopped/);
  assert.equal(port.tick, undefined, 'no loop was started');
});

test('a compile error and a refused line are reported', () => {
  const { out, send } = harness();
  send({ type: 'start', source: 'bad' });
  assert.deepEqual(out, [{ type: 'error', error: 'Error: compile: bad' }]);
  send({ type: 'start', source: 'ok' });
  send({ type: 'line', line: 'bogus you' });
  assert.deepEqual(out.at(-1), { type: 'error', error: 'unknown action' });
});

test('seats appear on the page as placed, seen from above', () => {
  const seats = [
    { name: 'a', side: 'bottom' },
    { name: 'b', side: 'bottom' },
    { name: 'c', side: 'top' },
    { name: 'd', side: 'top' },
    { name: 'e', side: 'right' },
    { name: 'f', side: 'right' },
  ];
  const names = (side) => seatsOnPage(seats, side).map((s) => s.name);
  assert.deepEqual(names('bottom'), ['a', 'b']);
  assert.deepEqual(names('top'), ['d', 'c'], 'the top row faces the bottom one');
  assert.deepEqual(names('right'), ['f', 'e'], 'the right column runs bottom to top');
  assert.deepEqual(names('left'), []);
});

test('pixels paint as opaque RGBA, transparent as the panel', () => {
  const data = new Uint8ClampedArray(8);
  paint(data, [0xff8000, -1]);
  assert.deepEqual([...data], [255, 128, 0, 255, 10, 10, 14, 255]);
});

test('a row paints at its offset and leaves the rest', () => {
  const data = new Uint8ClampedArray(12);
  paint(data, [0x0000ff], 2);
  assert.deepEqual([...data], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255]);
});

test('the chat keeps its last lines', () => {
  assert.deepEqual(appendLines(['a', 'b'], ['c'], 2), ['b', 'c']);
  assert.deepEqual(appendLines([], ['x']), ['x']);
});
