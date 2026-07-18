import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseDump, deriveRules } from '../gen_semantics.mjs';
import { readFileSync } from 'fs';

const dump = readFileSync(new URL('./fixtures/probe_dump_v1.txt', import.meta.url), 'utf8');

test('parses cases with full gate class names', () => {
  const t = parseDump(dump, 2);
  const eq = t.gates['BrickComponentType_WireGraph_Expr_CompareEqual'];
  assert.equal(eq.cases.length, 4);
  assert.deepEqual(eq.cases[1].inputs[1], { variant: 'str', value: '1' });
  // Comma-separated FormatText int rendering is normalized away.
  assert.deepEqual(eq.cases[2].inputs[1], { variant: 'int', value: '9007199254740992' });
  assert.equal(eq.cases[3].inputs[1], 'unwired');
});

test('rejects version mismatch', () => {
  assert.throws(() => parseDump(dump, 3), /probe version/i);
});

test('rejects chapter count mismatch', () => {
  const bad = dump.replace('BEGIN eq 5', 'BEGIN eq 6');
  assert.throws(() => parseDump(bad, 2), /chapter/i);
});

test('derives annihilator entailed by cases', () => {
  const t = parseDump(dump, 2);
  const rules = deriveRules(t.gates['BrickComponentType_WireGraph_Expr_LogicalAND'], 'BrickComponentType_WireGraph_Expr_LogicalAND');
  assert.ok(rules.some(r =>
    r.kind === 'annihilator' &&
    r.when.variant === 'bool' && r.when.value === 'false' &&
    r.result.value === 'false'));
});

// Contract test: guards the probe (probes/gate_semantics.ws) <-> parser
// boundary, not just the fixture above. Every line below is copied verbatim
// in *format* from a real caseLine(...) call in that probe — one per
// chapter (eq/bool/compare/select/branch/math) — with plausible values
// substituted, plus the two shapes called out for extra scrutiny: a unary
// Select line and an unwired binary operand.
const probeShapedDump = `PROBE gate_semantics v1
BEGIN eq 2
CASE CompareEqual int:1 int:1 -> true
CASE CompareEqual int:1 unwired -> false
END eq
BEGIN bool 1
CASE LogicalAND bool:true bool:false -> false
END bool
BEGIN compare 2
CASE CompareLessEqual int:1 int:1 -> true
CASE CompareGreaterEqual int:1 int:1 -> true
END compare
BEGIN select 1
CASE Select int:0 - -> 222
END select
BEGIN branch 1
CASE Branch bool:true - -> A
END branch
BEGIN math 1
CASE MathAdd int:1 int:2 -> 3
END math
PROBE done
`;

test('accepts a hand-built mini-dump spanning every probe family', () => {
  const t = parseDump(probeShapedDump, 1);

  const eq = t.gates['BrickComponentType_WireGraph_Expr_CompareEqual'];
  assert.equal(eq.cases.length, 2);
  assert.equal(eq.cases[1].inputs[1], 'unwired');

  const and = t.gates['BrickComponentType_WireGraph_Expr_LogicalAND'];
  assert.deepEqual(and.cases[0].output, { variant: 'bool', value: 'false' });

  // The corrected mappings (...CompareLessOrEqual / ...CompareGreaterOrEqual).
  assert.ok(t.gates['BrickComponentType_WireGraph_Expr_CompareLessOrEqual']);
  assert.ok(t.gates['BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual']);

  const select = t.gates['BrickComponentType_WireGraph_Expr_Select'];
  assert.equal(select.cases[0].inputs.length, 1);
  assert.deepEqual(select.cases[0].inputs[0], { variant: 'int', value: '0' });
  assert.deepEqual(select.cases[0].output, { variant: 'int', value: '222' });

  const branch = t.gates['BrickComponentType_WireGraph_Exec_Branch'];
  assert.equal(branch.cases[0].inputs.length, 1);
  assert.deepEqual(branch.cases[0].output, { variant: 'str', value: 'A' });

  const mathAdd = t.gates['BrickComponentType_WireGraph_Expr_MathAdd'];
  assert.deepEqual(mathAdd.cases[0].output, { variant: 'int', value: '3' });
});

test('whitelist: no false annihilators from coincidental case pools', () => {
  // `"a" > 1` false and `"a" > ""` false must NOT yield a `> str:a` law:
  // CompareGreater has no whitelisted candidates at all.
  const entry = { cases: [
    { inputs: [{variant:'str',value:'a'},{variant:'int',value:'1'}], output: {variant:'bool',value:'false'} },
    { inputs: [{variant:'str',value:'a'},{variant:'str',value:''}], output: {variant:'bool',value:'false'} },
  ], rules: [] };
  assert.deepEqual(deriveRules(entry, 'BrickComponentType_WireGraph_Expr_CompareGreater'), []);
});

test('whitelist: contradicting observation is a hard error', () => {
  const entry = { cases: [
    { inputs: [{variant:'bool',value:'false'},{variant:'bool',value:'true'}], output: {variant:'bool',value:'true'} },
  ], rules: [] };
  assert.throws(() => deriveRules(entry, 'BrickComponentType_WireGraph_Expr_LogicalAND'), /contradicted/);
});

test('raw game-log prefixes and interleaved noise are tolerated', () => {
  const noisy = [
    '[2026.07.18-01.00.00:000][ 62]LogBrickadia: PROBE gate_semantics v2',
    '[2026.07.18-01.00.00:010][ 63]LogBrickadia: BEGIN eq 1',
    '[2026.07.18-01.00.00:011][ 63]LogChat: someone said something',
    '[2026.07.18-01.00.00:020][ 64]LogBrickadia: CASE CompareEqual int:1 int:1 -> true',
    '[2026.07.18-01.00.00:030][ 65]LogBrickadia: END eq',
    '[2026.07.18-01.00.00:040][ 66]LogBrickadia: PROBE done',
  ].join('\n');
  const t = parseDump(noisy, 2);
  assert.equal(t.gates['BrickComponentType_WireGraph_Expr_CompareEqual'].cases.length, 1);
});
