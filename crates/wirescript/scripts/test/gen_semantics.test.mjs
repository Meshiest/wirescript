import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseDump, deriveRules } from '../gen_semantics.mjs';
import { readFileSync } from 'fs';

const dump = readFileSync(new URL('./fixtures/probe_dump_v1.txt', import.meta.url), 'utf8');
const dumpV3 = readFileSync(new URL('./fixtures/probe_dump_v3.txt', import.meta.url), 'utf8');

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

// ============================================================================
// v3: render / strings / compositeMath / compositeOps / deferredOps
// ============================================================================

test('v3: rejects a v2 dump under the v3 expectation and vice versa', () => {
  assert.throws(() => parseDump(dump, 3), /probe version/i);
  assert.throws(() => parseDump(dumpV3, 2), /probe version/i);
});

test('v3: render chapter lands in a dedicated top-level table, not under gates', () => {
  const t = parseDump(dumpV3, 3);
  assert.equal(t.gates['Render'], undefined);
  assert.equal(t.render['int:0'], '0');
  // Raw render text is stored VERBATIM — commas and internal spaces intact,
  // no int-comma-stripping or other gates-side normalization applied.
  assert.equal(t.render['int:1000'], '1,000');
  assert.equal(t.render['str:hello'], 'hello world');
  assert.equal(t.render['vector:Vec(1.0,2.0,3.0)'], '(X=1.000, Y=2.000, Z=3.000)');
  assert.equal(t.render['quat:Quat(0.0,0.0,0.707,0.707)'], '(X=0.000, Y=0.000, Z=0.707, W=0.707)');
});

test('v3: quoted string operands with internal spaces tokenize as one operand', () => {
  const t = parseDump(dumpV3, 3);
  const contains = t.gates['BrickComponentType_WireGraph_Expr_String_Contains'];
  assert.deepEqual(contains.cases[0].inputs[0], { variant: 'str', value: 'hello world' });
  assert.deepEqual(contains.cases[0].inputs[1], { variant: 'str', value: 'world' });
  // Contains is a bool-output gate (v3 BOOL_OUTPUT extension), not int/str.
  assert.deepEqual(contains.cases[0].output, { variant: 'bool', value: 'true' });
});

// Fix wave: the game's log channel strips trailing whitespace per line —
// an unquoted `CASE String_ToLower str:'  x  ' ->   x  ` would have
// silently lost its trailing spaces. Quote-delimiting the RESULT (fix 1;
// SINGLE quotes so the probe's double-quoted templates need no escaping)
// puts the closing quote, not a space, last on the line, and
// gen_semantics.mjs dequotes it back to the value verbatim — trailing
// spaces included.
test('v3: a quoted string RESULT with trailing spaces round-trips exactly', () => {
  const t = parseDump(dumpV3, 3);
  const lower = t.gates['BrickComponentType_WireGraph_Expr_String_ToLower'];
  assert.deepEqual(lower.cases[0].output, { variant: 'str', value: '  x  ' });
});

test('v3: string-RESULT gates (Concatenate/Substring/Replace/FormatText) dequote to the plain value', () => {
  const t = parseDump(dumpV3, 3);
  const concat = t.gates['BrickComponentType_WireGraph_Expr_String_Concatenate'];
  assert.deepEqual(concat.cases[0].output, { variant: 'str', value: 'ab' });
  const sub = t.gates['BrickComponentType_WireGraph_Expr_String_Substring'];
  assert.deepEqual(sub.cases[0].output, { variant: 'str', value: 'ell' });
  const repl = t.gates['BrickComponentType_WireGraph_Expr_String_Replace'];
  assert.deepEqual(repl.cases[0].output, { variant: 'str', value: 'hello there' });
  const fmt = t.gates['BrickComponentType_WireGraph_Expr_String_FormatText'];
  assert.deepEqual(fmt.cases[0].output, { variant: 'str', value: 'hi' });
});

// Mixed-format guard (fix 2): a v3 dump captured with the PRE-FIX probe has
// these same gates' results unquoted — hard error rather than silently
// accept a value that may have lost trailing whitespace in transit.
test('v3: a string-result gate CASE line without quotes is a hard error (mixed-format guard)', () => {
  const bad = dumpV3.replace(
    "CASE String_Concatenate str:a str:b -> 'ab'",
    'CASE String_Concatenate str:a str:b -> ab');
  assert.throws(() => parseDump(bad, 3), /quote-delimited/i);
});

test('v3: an unquoted render str: label is a hard error (mixed-format guard)', () => {
  const bad = dumpV3.replace(
    "CASE Render str:hello -> 'hello world'",
    'CASE Render str:hello -> hello world');
  assert.throws(() => parseDump(bad, 3), /quote-delimited/i);
});

// A dump captured with the short-lived escaped-DOUBLE-quote probe format is
// just as stale as an unquoted one — the output guard and the operand
// dequoter must both reject it loudly rather than silently accept.
test('v3: double-quote-delimited outputs/operands are a hard error (stale prior-format dump)', () => {
  const badOut = dumpV3.replace(
    "CASE String_Concatenate str:a str:b -> 'ab'",
    'CASE String_Concatenate str:a str:b -> "ab"');
  assert.throws(() => parseDump(badOut, 3), /quote-delimited/i);

  const badOperand = dumpV3.replace(
    "CASE String_ParseNumber str:'1.5' -> 1.5",
    'CASE String_ParseNumber str:"1.5" -> 1.5');
  assert.throws(() => parseDump(badOperand, 3), /stale dump format/i);
});

test('v3: variable operand arity — 3-operand Substring, 3-operand MakeVector', () => {
  const t = parseDump(dumpV3, 3);
  const sub = t.gates['BrickComponentType_WireGraph_Expr_String_Substring'];
  assert.equal(sub.cases[0].inputs.length, 3);
  assert.deepEqual(sub.cases[0].inputs[1], { variant: 'int', value: '1' });
  assert.deepEqual(sub.cases[0].inputs[2], { variant: 'int', value: '3' });

  const mkVec = t.gates['BrickComponentType_WireGraph_Expr_MakeVector'];
  assert.equal(mkVec.cases[0].inputs.length, 3);
  assert.deepEqual(mkVec.cases[0].output, { variant: 'vector', value: '(X=1.500, Y=-2.500, Z=0.750)' });
});

test('v3: compositeMath — vector operand labels transcribed verbatim, output tagged vector', () => {
  const t = parseDump(dumpV3, 3);
  const add = t.gates['BrickComponentType_WireGraph_Expr_MathAdd'];
  const vecCase = add.cases.find(c => c.inputs[0] !== 'unwired' && c.inputs[0].variant === 'vector');
  assert.deepEqual(vecCase.inputs[0], { variant: 'vector', value: 'Vec(0.5,0.25,-0.75)' });
  assert.deepEqual(vecCase.output, { variant: 'vector', value: '(X=0.750, Y=0.750, Z=0.000)' });
});

test('v3: composite operands do NOT hijack a bool-output gate\'s classification', () => {
  // Regression: CompareEqual on two `color` operands must still classify as
  // bool output — a first draft of the composite pass-through rule was a
  // blanket "any composite input -> composite output" check that wrongly
  // forced CompareEqual/VecDotProduct/VecMagnitude results to a composite
  // variant too; it must be scoped to the shared Math gates only.
  const t = parseDump(dumpV3, 3);
  const eq = t.gates['BrickComponentType_WireGraph_Expr_CompareEqual'];
  const colorCase = eq.cases.find(c => c.inputs[0] !== 'unwired' && c.inputs[0].variant === 'color');
  assert.deepEqual(colorCase.output, { variant: 'bool', value: 'true' });

  const dot = t.gates['BrickComponentType_WireGraph_Expr_VecDotProduct'];
  assert.equal(dot.cases[0].output.variant, 'float');

  const mag = t.gates['BrickComponentType_WireGraph_Expr_VecMagnitude'];
  assert.equal(mag.cases[0].output.variant, 'float');
});

test('v3: deferredOps is parsed/collected like any other chapter', () => {
  const t = parseDump(dumpV3, 3);
  assert.ok(t.gates['BrickComponentType_WireGraph_Expr_QuatSlerp']);
  assert.equal(t.gates['BrickComponentType_WireGraph_Expr_QuatSlerp'].cases.length, 1);
});

test('v3: hard-errors on a quoted string operand with an embedded quote/backslash', () => {
  const badLine = "CASE String_Length str:'a'b' -> 1";
  const badDump = [
    'PROBE gate_semantics v3',
    'BEGIN strings 1',
    badLine,
    'END strings',
    'PROBE done',
  ].join('\n');
  assert.throws(() => parseDump(badDump, 3), /escape rule/i);
});

test('v3: hard-errors on an unterminated quoted string operand', () => {
  const badDump = [
    'PROBE gate_semantics v3',
    'BEGIN strings 1',
    "CASE String_Length str:'unterminated -> 1",
    'END strings',
    'PROBE done',
  ].join('\n');
  assert.throws(() => parseDump(badDump, 3), /escape rule/i);
});

test('v3: chapter count mismatch is still detected for a new chapter name', () => {
  const bad = dumpV3.replace('BEGIN render 11', 'BEGIN render 12');
  assert.throws(() => parseDump(bad, 3), /chapter/i);
});

// Contract test: guards the probe (probes/gate_semantics.ws) <-> parser
// boundary for the v3 additions specifically — each line below is copied
// verbatim in *format* from a real caseLine(...)/render call in that probe.
const v3ShapedDump = `PROBE gate_semantics v3
BEGIN render 2
CASE Render int:7 -> 7
CASE Render vector:Vec(0.5,-1.25,1.0/3.0) -> (X=0.500, Y=-1.250, Z=0.333)
END render
BEGIN strings 2
CASE String_Length str:'  x  ' -> 5
CASE String_FormatText tmpl:0str -> 'hi'
END strings
BEGIN compositeMath 1
CASE MathModulo vector:Vec(0.5,0.25,-0.75) int:2 -> (X=0.500, Y=0.250, Z=-0.750)
END compositeMath
BEGIN compositeOps 1
CASE MakeColorSRGB int:255 int:128 int:0 int:255 -> (R=255, G=128, B=0, A=255)
END compositeOps
BEGIN deferredOps 1
CASE VecDistance vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> 0.6373774
END deferredOps
PROBE done
`;

test('v3 contract: probe-shaped lines for every new chapter parse correctly', () => {
  const t = parseDump(v3ShapedDump, 3);
  assert.equal(t.render['int:7'], '7');
  assert.equal(t.render['vector:Vec(0.5,-1.25,1.0/3.0)'], '(X=0.500, Y=-1.250, Z=0.333)');

  const len = t.gates['BrickComponentType_WireGraph_Expr_String_Length'];
  assert.deepEqual(len.cases[0].inputs[0], { variant: 'str', value: '  x  ' });

  const fmt = t.gates['BrickComponentType_WireGraph_Expr_String_FormatText'];
  assert.deepEqual(fmt.cases[0].inputs[0], { variant: 'tmpl', value: '0str' });
  // Quotes are transport armor, not value content — dequoted to plain "hi".
  assert.deepEqual(fmt.cases[0].output, { variant: 'str', value: 'hi' });

  const mod = t.gates['BrickComponentType_WireGraph_Expr_MathModulo'];
  assert.deepEqual(mod.cases[0].output, { variant: 'vector', value: '(X=0.500, Y=0.250, Z=-0.750)' });

  const srgb = t.gates['BrickComponentType_WireGraph_Expr_MakeColorSRGB'];
  assert.equal(srgb.cases[0].inputs.length, 4);
  assert.deepEqual(srgb.cases[0].output, { variant: 'color', value: '(R=255, G=128, B=0, A=255)' });

  const dist = t.gates['BrickComponentType_WireGraph_Expr_VecDistance'];
  assert.equal(dist.cases[0].output.variant, 'float');
});
