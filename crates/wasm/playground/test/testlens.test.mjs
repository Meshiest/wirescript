import { test } from 'node:test';
import assert from 'node:assert/strict';
import { testLenses, testReportRows } from '../editor.js';

const tests = [
  { name: 'doubles', kind: 'fn', startLine: 4 },
  { name: 'wrong', kind: 'mod', startLine: 9 },
];

test('a file with tests gets a run-all lens on top and one per test', () => {
  const lenses = testLenses(tests);
  assert.deepEqual(lenses.map(l => [l.line, l.title, l.names]), [
    [0, 'Run all tests (2)', ['doubles', 'wrong']],
    [4, 'Run test', ['doubles']],
    [9, 'Run test', ['wrong']],
  ]);
  assert.deepEqual(testLenses([]), []);
});

test('a lens shows its test last verdict', () => {
  const lenses = testLenses(tests, new Map([['doubles', true], ['wrong', false]]));
  assert.deepEqual(lenses.slice(1).map(l => l.title), ['Run test (passed)', 'Run test (failed)']);
});

test('a report lists each test, its chat, and problems the chat did not show', () => {
  const rows = testReportRows([
    { name: 'doubles', passed: true, problems: [], lines: [] },
    {
      name: 'wrong',
      passed: false,
      problems: ['FAIL main:9: double(2) expected 5, got 4', 'error: the test did not reach its end within 600 ticks'],
      lines: [
        { tick: 0, text: 'LOG main:8: checking' },
        { tick: 0, text: 'FAIL main:9: double(2) expected 5, got 4' },
      ],
    },
  ]);
  assert.deepEqual(rows.map(r => [r.kind, r.text]), [
    ['pass', 'PASS doubles'],
    ['fail', 'FAIL wrong'],
    ['line', 'LOG main:8: checking'],
    ['failLine', 'FAIL main:9: double(2) expected 5, got 4'],
    ['problem', 'error: the test did not reach its end within 600 ticks'],
  ]);
});
