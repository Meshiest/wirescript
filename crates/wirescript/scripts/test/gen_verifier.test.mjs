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
  assert.match(ws, /skipped 1 NaN-involved/);
  assert.match(ws, /total: int = 1/);
});

// ============================================================================
// v3: strings / compositeMath / compositeOps / deferredOps
// ============================================================================

test('v3: a unary CALL_FN gate (String_Length) generates a named call, not an operator', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_String_Length': {
        cases: [
          { inputs: [{ variant: 'str', value: 'hello' }], output: { variant: 'int', value: '5' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /Length\(Opaque\("hello"\)\)/);
  assert.match(ws, /== 5/);
});

test('v3: String_Concatenate uses the `..` infix operator', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_String_Concatenate': {
        cases: [
          { inputs: [{ variant: 'str', value: 'a' }, { variant: 'str', value: 'b' }],
            output: { variant: 'str', value: 'ab' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /Opaque\("a"\) \.\. Opaque\("b"\)/);
});

test('v3: deferredOps gates are skipped with a tally, regardless of value', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_VecMagnitude': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(1.0,0.0,0.0)' }], output: { variant: 'float', value: '1.0' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 deferredOps/);
  assert.match(ws, /total: int = 0/);
});

test('v3: String_FormatText is skipped — its template is a synthetic label, not a real value', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_String_FormatText': {
        cases: [
          { inputs: [{ variant: 'tmpl', value: '0str' }], output: { variant: 'str', value: 'hi' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 String_FormatText/);
  assert.match(ws, /total: int = 0/);
});

test('v3: a composite operand whose label is not a constructor call is skipped with a tally', () => {
  // Mirrors probes/gate_semantics.ws's quat shorthand labels ("90Z") — these
  // describe the value, they are not themselves valid wirescript syntax.
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_RotateVector': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(1,0,0)' }, { variant: 'quat', value: '90Z' }],
            output: { variant: 'vector', value: '(X=0.000, Y=1.000, Z=0.000)' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 cases with a non-reconstructable composite operand label/);
  assert.match(ws, /total: int = 0/);
});

// Fix wave (Task 3): the old value-EQ bootstrap had to "reconstruct" a
// composite literal from rendered text and could skip with a tally when
// that failed (e.g. non-numeric/garbled text). Rendered-TEXT equality never
// reconstructs anything — the recorded text is asserted verbatim — so even
// a nonsense recorded value is fully assertable; there is no more
// "unparseable" case to skip.
test('v3: a composite output is assertable via rendered-text equality even when its recorded text is not a reconstructable literal', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_MakeRotation': {
        cases: [
          { inputs: [{ variant: 'float', value: '30.0' }, { variant: 'float', value: '60.0' }, { variant: 'float', value: '90.0' }],
            output: { variant: 'rotator', value: 'garbled' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /total: int = 1/);
  assert.match(ws, /let lr1 = "\$\{got1\}"/);
  assert.match(ws, /if lr1 == "garbled" \{ pass = pass \+ 1 \}/);
});

// wave-2 review fix: a composite-output case whose recorded text is EXACTLY
// "" would previously generate `lr1 == ""`, which ALWAYS passes for a
// rotator/quat/color result (they never render through FormatText — the
// live in-circuit text is ALSO always "") regardless of whether the
// underlying value is right. This is now routed to a loud, separately
// tallied skip instead. Distinguish from the 'garbled' test above, whose
// recorded text is non-empty (and therefore still fully assertable).
test('v3: a composite output whose recorded text is EXACTLY "" is skipped as a vacuous assertion (blank==blank)', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_MakeRotation': {
        cases: [
          { inputs: [{ variant: 'float', value: '30.0' }, { variant: 'float', value: '60.0' }, { variant: 'float', value: '90.0' }],
            output: { variant: 'rotator', value: '' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 blank-render/);
  assert.match(ws, /total: int = 0/);
});

test('v3: NaN skip generalizes to a composite operand (not just a float output)', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_MathAdd': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(NaN,1,2)' }, { variant: 'vector', value: 'Vec(1,1,1)' }],
            output: { variant: 'vector', value: '(X=NaN, Y=2.000, Z=3.000)' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /skipped 1 NaN-involved/);
  assert.match(ws, /total: int = 0/);
});

test('v3: composite-output cases wrap the GOT expression in Opaque(...) and assert via rendered-text equality', () => {
  // A real `just check` run against a bare `got == Vec(...)` comparison fails
  // with WS004 ("no overload for '==' on Float, Vector" / "... Vector,
  // Vector") — Opaque(x) + Opaque(y)'s STATIC result type resolves to the
  // first-matching numeric overload even though the gate itself carries a
  // composite-variant wire at runtime. Wrapping `got` in an outer
  // Opaque(...) is what actually compiles AND makes it render its true
  // (vector) form when interpolated into `lr1` below — this locks both in.
  //
  // Fix wave (Task 3): the comparison itself is now RENDERED-TEXT equality
  // (`lr1 == "<recorded text>"`), not a value EQ against a reconstructed
  // `Vec(...)` literal — a true Z of -0.5625 (not exactly reconstructable
  // from a 3-decimal-rounded "Z=-0.562"/"Z=-0.563") would false-FAIL under
  // the old approach but is asserted correctly here since no reconstruction
  // is attempted.
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_MathAdd': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(0.5,0.25,-0.75)' }, { variant: 'vector', value: 'Vec(0.25,0.5,0.75)' }],
            output: { variant: 'vector', value: '(X=0.750, Y=0.750, Z=0.000)' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /let got1 = Opaque\(Opaque\(Vec\(0\.5, 0\.25, -0\.75\)\) \+ Opaque\(Vec\(0\.25, 0\.5, 0\.75\)\)\)/);
  assert.match(ws, /let lr1 = "\$\{got1\}"/);
  assert.match(ws, /if lr1 == "\(X=0\.750, Y=0\.750, Z=0\.000\)" \{ pass = pass \+ 1 \}/);
});

test('v3: a compositeOps float-output gate (VecDotProduct) also asserts via rendered-text equality, not value EQ', () => {
  // Task 3's rule is "ALL composite-output cases AND ALL float-output cases
  // in the compositeMath/compositeOps chapters" — VecDotProduct's output is
  // a plain float, not a composite variant, but it's still a
  // compositeOps-chapter gate whose value derives from the same probed
  // vector components, so it gets the same treatment (e.g. 0.5*0.25 +
  // 0.25*0.5 + -0.75*0.75 = -0.3125, rendered "-0.312"/"-0.313" — not
  // exactly reconstructable either).
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_VecDotProduct': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(0.5,0.25,-0.75)' }, { variant: 'vector', value: 'Vec(0.25,0.5,0.75)' }],
            output: { variant: 'float', value: '-0.312' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  // Float (non-composite) output — GOT is NOT Opaque-wrapped (its static
  // type already resolves concretely).
  assert.match(ws, /let got1 = Dot\(Opaque\(Vec\(0\.5, 0\.25, -0\.75\)\), Opaque\(Vec\(0\.25, 0\.5, 0\.75\)\)\)/);
  assert.match(ws, /let lr1 = "\$\{got1\}"/);
  assert.match(ws, /if lr1 == "-0\.312" \{ pass = pass \+ 1 \}/);
});

test('v3: composite operand labels widen bare integer components to float literals', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_VecScale': {
        cases: [
          { inputs: [{ variant: 'vector', value: 'Vec(1,0,0)' }, { variant: 'float', value: '2.0' }],
            output: { variant: 'vector', value: '(X=2.000, Y=0.000, Z=0.000)' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /ScaleVec\(Opaque\(Vec\(1\.0, 0\.0, 0\.0\)\), Opaque\(2\.0\)\)/);
});

test('v3: a bool-output gate with composite operands (composite EQ bootstrap) is NOT double-Opaque-wrapped', () => {
  const t = {
    build: 'b', probeVersion: 3,
    gates: {
      'BrickComponentType_WireGraph_Expr_CompareEqual': {
        cases: [
          { inputs: [{ variant: 'color', value: 'Color(1.0,0.5,0.25)' }, { variant: 'color', value: 'Color(1.0,0.5,0.25)' }],
            output: { variant: 'bool', value: 'true' } },
        ], rules: [],
      },
    },
  };
  const ws = generateVerifier(t);
  assert.match(ws, /let got1 = Opaque\(Color\(1\.0, 0\.5, 0\.25\)\) == Opaque\(Color\(1\.0, 0\.5, 0\.25\)\)/);
  assert.match(ws, /if got1 == true/);
});
