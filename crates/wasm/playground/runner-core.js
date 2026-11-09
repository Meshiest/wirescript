// The runner's simulation side, shared by the playground's worker and the
// VS Code extension's: a 60 Hz loop around the wasm `Runner`, and the key
// mapping that turns held keys into world script lines. No DOM here.
//
// Messages in:  { type: 'start', source, moduleName, filesJson, prefabsJson }
//               { type: 'line', line }     a world script line, e.g. `sit you p1`
//               { type: 'stop' }
// Messages out: { type: 'layout', layout } once, after a successful start
//               { type: 'update', update } each tick something changed
//               { type: 'error', error }   a compile error or a line refused

/** The local player's script name. */
export const PLAYER = 'you';

/** Ticks per second, as the game runs. */
export const TICK_RATE = 60;

/** Ticks between two reports when nothing changes. */
const QUIET_REPORT = 30;

// Each axis key (a `KeyboardEvent.code`: `KeyW` is the W key, and so on, so
// these are W A S D and Space): the InputReader axis it drives and its
// direction.
const AXIS_KEYS = {
  KeyW: ['Forward', 1],
  KeyS: ['Forward', -1],
  KeyD: ['Right', 1],
  KeyA: ['Right', -1],
  Space: ['Up', 1],
};

// Each button key: the input it holds.
const BUTTON_KEYS = {
  KeyE: 'E',
  KeyQ: 'Q',
  KeyC: 'C',
};

// Mouse buttons by `MouseEvent.button`.
const MOUSE_BUTTONS = { 0: 'LeftMouse', 2: 'RightMouse' };

/** Whether `code` (a `KeyboardEvent.code`) is one the runner reads. */
export function isRunnerKey(code) {
  return code in AXIS_KEYS || code in BUTTON_KEYS || code === 'Escape';
}

/**
 * The held keys of the local player in a seat, turned into the script lines
 * that tell the world. Each method returns the lines to send, in order.
 */
export class KeyState {
  constructor(player = PLAYER) {
    this.player = player;
    this.held = new Set();
  }

  /** The value an axis has with the keys now held. */
  axis(name) {
    let v = 0;
    for (const [code, [axis, dir]] of Object.entries(AXIS_KEYS)) {
      if (axis === name && this.held.has(code)) v += dir;
    }
    return v;
  }

  /** `code` went down (`down`) or up. */
  key(code, down) {
    if (code in AXIS_KEYS) {
      const axis = AXIS_KEYS[code][0];
      const before = this.axis(axis);
      if (down) this.held.add(code);
      else this.held.delete(code);
      const after = this.axis(axis);
      return before === after ? [] : [`input ${this.player} ${axis} ${after}`];
    }
    if (code in BUTTON_KEYS) return this.button(code, BUTTON_KEYS[code], down);
    return [];
  }

  /** Mouse button `button` (`MouseEvent.button`) went down or up. */
  mouse(button, down) {
    const name = MOUSE_BUTTONS[button];
    return name ? this.button(`Mouse${button}`, name, down) : [];
  }

  button(code, name, down) {
    if (down === this.held.has(code)) return [];
    if (down) this.held.add(code);
    else this.held.delete(code);
    return [`input ${this.player} ${name} ${down ? 1 : 0}`];
  }

  /** Let go of everything held: the lines that zero it all. */
  release() {
    const lines = [];
    for (const code of [...this.held]) {
      if (code.startsWith('Mouse')) lines.push(...this.mouse(Number(code.slice(5)), false));
      else lines.push(...this.key(code, false));
    }
    return lines;
  }
}

/**
 * Run the loop on a message port: `port.onmessage` receives the messages in,
 * `port.postMessage` sends the ones out. `Runner` is the wasm class.
 */
export function serveRunner(port, Runner, timers = globalThis) {
  let runner = null;
  let timer = null;
  const stop = () => {
    if (timer !== null) timers.clearInterval(timer);
    timer = null;
    runner?.free?.();
    runner = null;
  };
  let seatedBefore = null;
  let quiet = 0;
  const tick = (n) => {
    let update;
    try {
      update = JSON.parse(runner.step(n));
    } catch (e) {
      port.postMessage({ type: 'error', error: `the runner stopped: ${e}` });
      stop();
      return;
    }
    const seated = JSON.stringify(update.seated);
    const changed = update.chat.length || update.displays.length || update.error || seated !== seatedBefore;
    seatedBefore = seated;
    // A tick where nothing changed still reports now and then, so the page's
    // tick count moves.
    if (changed || ++quiet >= QUIET_REPORT) {
      quiet = 0;
      port.postMessage({ type: 'update', update });
    }
    if (update.error) stop();
  };
  port.onmessage = (event) => {
    const msg = event.data ?? event;
    if (msg.type === 'start') {
      stop();
      seatedBefore = null;
      quiet = 0;
      try {
        runner = new Runner(msg.source, msg.moduleName, msg.filesJson, msg.prefabsJson);
      } catch (e) {
        port.postMessage({ type: 'error', error: String(e) });
        return;
      }
      port.postMessage({ type: 'layout', layout: JSON.parse(runner.layout()) });
      tick(1);
      if (runner) timer = timers.setInterval(() => runner && tick(1), 1000 / TICK_RATE);
    } else if (msg.type === 'line' && runner) {
      const problem = runner.push(msg.line);
      if (problem) port.postMessage({ type: 'error', error: problem });
    } else if (msg.type === 'stop') {
      stop();
    }
  };
}
