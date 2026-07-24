import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

// files.js talks to `localStorage` directly — shim it before importing.
const store = new Map();
globalThis.localStorage = {
  getItem: (k) => (store.has(k) ? store.get(k) : null),
  setItem: (k, v) => store.set(k, String(v)),
  removeItem: (k) => store.delete(k),
  clear: () => store.clear(),
};

const files = await import('../files.js');

beforeEach(() => {
  store.clear();
  files.writeFile('main.ws', 'main content');
  files.writeFile('util.ws', 'util content');
  files.setActiveFile('main.ws');
});

test('renaming the active file keeps it active', () => {
  // The rename writes the files object before consulting the active pointer;
  // getActiveFile() then saw a stale pointer, healed it to the alphabetically
  // first file, and the never-matching oldName check skipped the re-point.
  // The editor kept showing the renamed content while active said util.ws —
  // the next autosave clobbered util.ws with it.
  assert.equal(files.renameFile('main.ws', 'zzz.ws'), true);
  assert.equal(files.getActiveFile(), 'zzz.ws');
  assert.equal(files.readFile('zzz.ws'), 'main content');
  assert.equal(files.readFile('util.ws'), 'util content');
  assert.equal(files.readFile('main.ws'), null);
});

test('renaming a non-active file leaves the active pointer alone', () => {
  assert.equal(files.renameFile('util.ws', 'aaa.ws'), true);
  assert.equal(files.getActiveFile(), 'main.ws');
  assert.equal(files.readFile('aaa.ws'), 'util content');
});

test('renaming onto an existing name is refused and changes nothing', () => {
  assert.equal(files.renameFile('util.ws', 'main.ws'), false);
  assert.equal(files.readFile('main.ws'), 'main content');
  assert.equal(files.readFile('util.ws'), 'util content');
  assert.equal(files.getActiveFile(), 'main.ws');
});

test('rename to the same name is a no-op success', () => {
  assert.equal(files.renameFile('main.ws', 'main.ws'), true);
  assert.equal(files.getActiveFile(), 'main.ws');
  assert.equal(files.readFile('main.ws'), 'main content');
});
