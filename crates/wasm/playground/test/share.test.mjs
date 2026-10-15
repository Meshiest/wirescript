import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decodeHash, encodeFiles, encodeToString } from '../share.js';

test('a single source round-trips as the bare hash', async () => {
  const src = '@left in a: bool\n@right out b = a\n// é ☃\n';
  const hash = await encodeToString(src);
  assert.match(hash, /^[A-Za-z0-9_-]+$/);
  assert.equal(await decodeHash(hash), src);
});

test('a set of files round-trips with its active file', async () => {
  const files = { 'door.ws': 'import { escape } from "sanitize"\n', 'sanitize.ws': 'mod escape(t: string) -> string { return t }\n' };
  const hash = await encodeFiles(files, 'door.ws');
  assert.ok(hash.startsWith('2.'));
  assert.deepEqual(await decodeHash(hash), { files, open: 'door.ws' });
});

test('an unknown active file falls back to the first file', async () => {
  const hash = await encodeFiles({ 'a.ws': 'x' }, 'missing.ws');
  assert.equal((await decodeHash(hash)).open, 'a.ws');
});

test('a hostile payload is refused, not written', async () => {
  const { encodeToString } = await import('../share.js');
  const as = async (payload) => decodeHash('2.' + (await encodeToString(JSON.stringify(payload))));
  // `open` naming an inherited property falls back to a real file.
  assert.equal((await as({ files: { 'a.ws': 'x' }, open: 'toString' })).open, 'a.ws');
  assert.equal((await as({ files: { 'a.ws': 'x' }, open: '__proto__' })).open, 'a.ws');
  // No files, an array, a non-.ws name or a folder path is no payload at all.
  assert.equal(await as({ files: {} }), null);
  assert.equal(await as({ files: ['x'] }), null);
  assert.equal(await as({ files: { toString: 'x' } }), null);
  assert.equal(await as({ files: { '../a.ws': 'x' } }), null);
  assert.equal(await as({ files: { 'a.ws': 5 } }), null);
});

test('garbage and empty hashes read as nothing', async () => {
  assert.equal(await decodeHash(''), null);
  assert.equal(await decodeHash('2.not-gzip'), null);
});
