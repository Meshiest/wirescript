import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { mdLines, opt, writeOrCheck } from '../lib.mjs';

test('a fenced block opens at any indent and closes at the same one', () => {
  const md = 'text\n  ```wirescript ignore\n  var x\n```\n  ```\nafter\r\n';
  const lines = [...mdLines(md)];
  assert.deepEqual(
    lines.map((l) => [l.line, l.fence, l.fenced]),
    [
      ['text', false, false],
      ['  ```wirescript ignore', true, false],
      ['  var x', false, true],
      ['```', false, true],
      ['  ```', true, false],
      ['after', false, false],
      ['', false, false],
    ],
  );
  assert.equal(lines[2].indent, '  ');
  assert.equal(lines[2].info, 'wirescript ignore');
});

test('a check reports a stale file and leaves it alone', () => {
  const path = join(mkdtempSync(join(tmpdir(), 'lib-test-')), 'out.txt');
  writeFileSync(path, 'old');
  const quiet = console.error;
  console.error = () => {};
  try {
    assert.equal(writeOrCheck(path, 'new', { check: true }), false);
  } finally {
    console.error = quiet;
  }
  assert.equal(readFileSync(path, 'utf8'), 'old');
  assert.equal(writeOrCheck(path, 'new', { check: false }), true);
  assert.equal(writeOrCheck(path, 'new', { check: true }), true);
});

test('an option takes the value after it, if any', () => {
  assert.equal(opt('--out', 'd', ['--out', 'x', 'dump']), 'x');
  assert.equal(opt('--out', 'd', ['dump', '--out']), 'd');
  assert.equal(opt('--out', 'd', ['dump']), 'd');
});
