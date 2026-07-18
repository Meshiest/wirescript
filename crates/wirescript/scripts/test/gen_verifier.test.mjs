import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateVerifier } from '../gen_verifier.mjs';

const table = {
  build: 'testbuild', probeVersion: 1,
  gates: {
    'BrickComponentType_WireGraph_Expr_CompareEqual': {
      cases: [
        { inputs: [{ variant: 'int', value: '1' }, { variant: 'str', value: '1' }],
          output: { variant: 'bool', value: 'false' } },
      ], rules: [],
    },
    'BrickComponentType_WireGraph_Exec_Branch': {
      cases: [
        { inputs: [{ variant: 'int', value: '0' }], output: { variant: 'str', value: 'B' } },
      ], rules: [],
    },
  },
};

test('emits header, opaque leaves, eq assertion, branch if-pattern', () => {
  const ws = generateVerifier(table);
  assert.match(ws, /VERIFIER v1 table testbuild/);
  assert.match(ws, /Opaque\(1\) == Opaque\("1"\)/);
  assert.match(ws, /== false/);                 // asserted against expected literal
  assert.match(ws, /if Opaque\(0\)/);           // Branch case → statement if
  assert.match(ws, /VERIFY \$\{pass\}\/\$\{total\}/);
  assert.match(ws, /let grid = ReadBrickGrid\(\)/);
});

test('unwired cases are skipped with a comment', () => {
  const t2 = structuredClone(table);
  t2.gates['BrickComponentType_WireGraph_Expr_CompareEqual'].cases.push(
    { inputs: [{ variant: 'int', value: '1' }, 'unwired'], output: { variant: 'bool', value: 'false' } });
  const ws = generateVerifier(t2);
  assert.match(ws, /skipped 1 unwired/);
});

test('LogicalXOR lowers to the ^^ operator, not !=', () => {
  const t3 = {
    build: 'testbuild', probeVersion: 1,
    gates: {
      'BrickComponentType_WireGraph_Expr_LogicalXOR': {
        cases: [
          { inputs: [{ variant: 'bool', value: 'true' }, { variant: 'bool', value: 'false' }],
            output: { variant: 'bool', value: 'true' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t3);
  assert.match(ws, /Opaque\(true\) \^\^ Opaque\(false\)/);
});

test('CompareLessOrEqual / CompareGreaterOrEqual classes map to <= / >=', () => {
  const t4 = {
    build: 'testbuild', probeVersion: 1,
    gates: {
      'BrickComponentType_WireGraph_Expr_CompareLessOrEqual': {
        cases: [
          { inputs: [{ variant: 'int', value: '1' }, { variant: 'int', value: '2' }],
            output: { variant: 'bool', value: 'true' } },
        ], rules: [],
      },
      'BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual': {
        cases: [
          { inputs: [{ variant: 'int', value: '2' }, { variant: 'int', value: '1' }],
            output: { variant: 'bool', value: 'true' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t4);
  assert.match(ws, /Opaque\(1\) <= Opaque\(2\)/);
  assert.match(ws, /Opaque\(2\) >= Opaque\(1\)/);
});

test('hostile string content is escaped, never breaks generation', () => {
  const t = {
    build: 'esc', probeVersion: 1,
    gates: {
      'BrickComponentType_WireGraph_Expr_CompareEqual': {
        cases: [
          { inputs: [{ variant: 'str', value: 'a"b' }, { variant: 'str', value: '${x}' }],
            output: { variant: 'bool', value: 'false' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  const BS = String.fromCharCode(92);
  assert.ok(ws.includes(BS + '"'), 'quote must be escaped in emitted literal');
  assert.ok(ws.includes(BS + '${'), 'interpolation opener must be neutralized');
  assert.ok(!ws.includes('Opaque("a"b")'), 'raw quote must not appear unescaped');
});

test('NaN-expected cases are skipped with a tally', () => {
  const t = {
    build: 'nan', probeVersion: 1,
    gates: {
      'BrickComponentType_WireGraph_Expr_MathAdd': {
        cases: [
          { inputs: [{ variant: 'float', value: 'nan' }, { variant: 'float', value: '1.0' }],
            output: { variant: 'float', value: 'nan' } },
          { inputs: [{ variant: 'float', value: '1.0' }, { variant: 'float', value: '2.0' }],
            output: { variant: 'float', value: '3.0' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 NaN-expected/);
  assert.match(ws, /total: int = 1/);
});
