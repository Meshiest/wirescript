#!/usr/bin/env node
// Generate probes/verify_semantics.ws from data/gate_semantics.json: an
// in-game circuit that re-asserts every certified case with real gates.
//   node scripts/gen_verifier.mjs [--table data/gate_semantics.json] [--out probes/verify_semantics.ws]
import { readFileSync, writeFileSync } from 'fs';
import { pathToFileURL, fileURLToPath } from 'url';
import { dirname, join } from 'path';

// Defaults are anchored to this script's own directory (crates/wirescript/),
// not process.cwd() — see gen_semantics.mjs for the rationale.
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const DEFAULT_TABLE = join(SCRIPT_DIR, '..', 'data', 'gate_semantics.json');
const DEFAULT_VERIFIER_OUT = join(SCRIPT_DIR, '..', 'probes', 'verify_semantics.ws');

const SHORT = cls => cls.replace(/^BrickComponentType_WireGraph_(Expr|Exec)_/, '');

// Escape arbitrary text for INSIDE a wirescript double-quoted interpolated
// string: backslash, quote, and the interpolation opener (lexer supports
// \, \", \$; JSON.stringify is unsafe here — its \uXXXX escapes are not
// valid wirescript escapes).
function wsEscape(s) {
  return String(s)
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
    .replace(/\$\{/g, '\\${')
    .replace(/\n/g, '\\n')
    .replace(/\t/g, '\\t')
    .replace(/\r/g, '\\r');
}
function wsString(s) {
  return '"' + wsEscape(s) + '"';
}

// v3: the four composite (non-scalar) wire variants.
const COMPOSITE_VARIANTS = new Set(['vector', 'rotator', 'color', 'quat']);

// Composite operand LABELS are hardcoded verbatim wirescript constructor
// syntax by convention (see probes/gate_semantics.ws file banner), e.g.
// `vector:Vec(0.5,0.25,-0.75)` — so the label text is (almost) already the
// literal we want to emit. The one wrinkle: constructors take floats, and a
// label may write a component as a bare integer (`Vec(1,0,0)`) — widen those
// so the generated source always typechecks.
function reformatCtorLiteral(text) {
  const m = text.match(/^(\w+)\((.*)\)$/);
  if (!m) return text;
  const [, name, argsStr] = m;
  const args = argsStr.split(',').map(a => {
    a = a.trim();
    return /^-?\d+$/.test(a) ? `${a}.0` : a;
  });
  return `${name}(${args.join(', ')})`;
}

// Not every composite label is a reconstructable constructor call: quat
// labels in this probe are shorthand description text (`quat:90Z`,
// `quat:Quat90Z`), never the literal value fed to Opaque(...) (see e.g.
// rotateVectorCases / compositeQuatOpsCases / cmpEqQuatL in
// probes/gate_semantics.ws). Only labels shaped like an actual constructor
// call can be echoed back as source.
function isCtorLabel(value) {
  return /^(Vec|Rotation|Quat|Color)\(.*\)$/.test(value);
}

// Wirescript literal for a probed operand.
function lit({ variant, value }) {
  if (COMPOSITE_VARIANTS.has(variant)) return reformatCtorLiteral(value);
  if (variant === 'str') return wsString(value);
  if (variant === 'float') {
    if (/nan/i.test(value)) return '(0.0 / 0.0)';
    if (/^-?inf$/i.test(value)) return `(${value.startsWith('-') ? '-1.0' : '1.0'} / 0.0)`;
    return value.includes('.') ? value : `${value}.0`;
  }
  return value; // int, bool
}

// v3 fix wave (Task 3): gate short names whose FLOAT output lives in the
// compositeOps chapter (compositeMath's shared Math* gates are ALWAYS
// composite-output when a composite operand is present — see
// gen_semantics.mjs's SHARED_COMPOSITE_MATH_GATES — so there are no
// float-only compositeMath cases to list separately here). These operate on
// the SAME probed vector/quat component values (halves/quarters etc.) whose
// products don't generally land on an exact 3-decimal boundary (e.g.
// 0.5*0.25 + 0.25*0.5 + -0.75*0.75 = -0.5625, which the table's
// 3-decimal-rounded rendered text cannot round-trip to). A value EQ
// reconstructed from that rounded text against the live full-precision
// result is a false FAIL — see usesRenderedTextAssert below.
const COMPOSITE_CHAPTER_FLOAT_SHORT_NAMES = new Set([
  'VecDotProduct', 'VecMagnitudeSquared', 'VecDistanceSquared', 'QuatDotProduct',
]);

// True for every case that must be asserted by RENDERED-TEXT equality
// instead of value EQ: all composite-output cases (vector/rotator/color/
// quat — their rendered text IS the recorded contract, no reconstruction
// needed/attempted) and the compositeOps chapter's float-output gates
// above. Everything else (scalar int/bool, and the strings chapter's
// string-output gates) keeps using value EQ against `lit(c.output)`.
function usesRenderedTextAssert(short, outputVariant) {
  return COMPOSITE_VARIANTS.has(outputVariant) || COMPOSITE_CHAPTER_FLOAT_SHORT_NAMES.has(short);
}

// A probed value never carries "nan" outside a float — check both operand
// and output.
function looksLikeNan(operand) {
  return !!operand && operand !== 'unwired' && /nan/i.test(operand.value);
}

// NOTE: LogicalXOR maps to the dedicated `^^` operator, NOT `!=` — `!=`
// lowers to CompareNotEqual, a different gate class (see
// probes/gate_semantics.ws). CompareLessOrEqual / CompareGreaterOrEqual
// (WITH "Or") match the SHORT() spelling of the real gate class names, per
// Task 4's landed GATE_CLASS table in gen_semantics.mjs.
const BIN_OP = {
  CompareEqual: '==', CompareNotEqual: '!=',
  LogicalAND: '&&', LogicalOR: '||', LogicalXOR: '^^',
  CompareLess: '<', CompareLessOrEqual: '<=',
  CompareGreater: '>', CompareGreaterOrEqual: '>=',
  MathAdd: '+', MathSubtract: '-', MathMultiply: '*',
  MathDivide: '/', MathModulo: '%',
  // v3: `..` is String_Concatenate's dedicated infix operator (see
  // probes/gate_semantics.ws) — MathAdd/etc. above are reused verbatim for
  // the compositeMath chapter's vector operands, no new entries needed there.
  String_Concatenate: '..',
};

// v3: gates reached via a named builtin call rather than an infix operator
// (function name -> wirescript call), argument order matching the table's
// `inputs` order (which mirrors the probe's own call-argument order).
const CALL_FN = {
  VecScale: 'ScaleVec', VecDotProduct: 'Dot', VecCrossProduct: 'Cross',
  VecMagnitudeSquared: 'MagnitudeSq', VecDistanceSquared: 'DistanceSq',
  RotateVector: 'Rotate', InvertRotation: 'Invert', QuatDotProduct: 'QuatDot',
  MakeVector: 'Vec', MakeRotation: 'Rotation', MakeQuaternion: 'Quat',
  MakeColor: 'Color', MakeColorSRGB: 'ColorSRGB', MakeColorHex: 'ColorHex',
  ColorToHex: 'ToHex',
  String_Length: 'Length', String_ToLower: 'ToLower', String_ToUpper: 'ToUpper',
  String_Trim: 'Trim', String_Contains: 'Contains', String_StartsWith: 'StartsWith',
  String_EndsWith: 'EndsWith', String_Substring: 'Substring', String_Find: 'Find',
  String_Replace: 'Replace', String_ParseInt: 'ParseInt', String_ParseNumber: 'ParseNumber',
};

// v3: deferredOps gate short names — probed/collected (see gen_semantics.mjs
// GATE_CLASS) but never folded in this wave, so never asserted here either.
const DEFERRED_SHORT_NAMES = new Set([
  'VecMagnitude', 'VecNormalize', 'VecDistance', 'QuatSlerp', 'QuatFromAxisAngle',
  'QuatAngleBetween', 'QuatBetween', 'DirectionToRotation', 'RotationToDirection',
  'ColorBlend',
]);

export function generateVerifier(table) {
  const out = [];
  const cases = [];
  let skippedUnwired = 0;
  let skippedNan = 0;
  let skippedDeferred = 0;
  let skippedTemplate = 0;
  let skippedNonCtorInput = 0;
  let skippedBlankRender = 0;
  for (const [cls, entry] of Object.entries(table.gates)) {
    const short = SHORT(cls);
    for (const c of entry.cases) {
      // deferredOps chapter (v3): probed/collected only, never folded, never
      // asserted — gate class alone identifies these (they're not reused by
      // any other chapter). Checked BEFORE the blank-render check below so
      // deferredOps' own tally/count stays exactly what it was — some
      // deferredOps gates (QuatSlerp/QuatFromAxisAngle/QuatBetween/
      // DirectionToRotation/ColorBlend) ALSO happen to render blank, but
      // they're already fully accounted for by this chapter-wide skip.
      if (DEFERRED_SHORT_NAMES.has(short)) { skippedDeferred++; continue; }
      // wave-2 review fix: a composite-output case whose RECORDED text is
      // EXACTLY "" can never be a meaningful assertion — rotator/quat/color
      // never render through FormatText (certified, see gen_semantics.mjs's
      // `render` section and eval.rs's `render_for_format`), so the live
      // in-circuit assertion below would compare "" == "" and ALWAYS pass
      // regardless of whether the underlying value is correct
      // (MakeRotation/MakeQuaternion/MakeColor/MakeColorSRGB/MakeColorHex/
      // InvertRotation — 6 cases at time of writing). Route these to a
      // loud, separately-tallied skip instead of emitting a vacuous
      // assertion — this is what actually closes the hole structurally
      // (rather than relying on reviewers to notice); see eval.rs's
      // BLANK_RENDER_REFUSED / the transitive-certification comments on
      // make_quaternion/make_color for how these are validated instead.
      if (COMPOSITE_VARIANTS.has(c.output.variant) && c.output.value === '') {
        skippedBlankRender++;
        console.error(`gen_verifier: skipping ${short} — composite output renders blank (${c.output.variant}); an assertion here would be vacuous (blank==blank proves nothing)`);
        continue;
      }
      // String_FormatText's only recorded operand is a synthetic label
      // ("tmpl:0str") identifying which template shape was probed — the
      // actual template string used isn't in the table, so there is no way
      // to reconstruct the real Fmt(...) call from it.
      if (short === 'String_FormatText') { skippedTemplate++; continue; }
      if (c.inputs.some(i => i === 'unwired')) { skippedUnwired++; continue; }
      // An EQ assertion against an expected NaN can never pass under IEEE
      // compare (and the game's NaN== semantics are exactly what the probe
      // certifies) — skip rather than generate an always-red assertion.
      // v3: this can show up in a composite operand too (e.g. the sanitize
      // case `Vec(NaN,1,2) + Vec(1,1,1)`), so check every operand, not just
      // a float-variant output.
      if (looksLikeNan(c.output) || c.inputs.some(looksLikeNan)) { skippedNan++; continue; }
      // v3: a composite operand's label must be a reconstructable
      // constructor call (see isCtorLabel) to be echoed back as source —
      // some quat labels in this probe are shorthand description text, not
      // literal values (see reformatCtorLiteral's comment).
      const badInput = c.inputs.find(i => i && i !== 'unwired' && COMPOSITE_VARIANTS.has(i.variant) && !isCtorLabel(i.value));
      if (badInput) {
        skippedNonCtorInput++;
        console.error(`gen_verifier: skipping ${short} — composite operand label is not a reconstructable constructor call: ${JSON.stringify(badInput.value)}`);
        continue;
      }
      const renderTextAssert = usesRenderedTextAssert(short, c.output.variant);
      // Render-text-assert cases compare the LIVE result's OWN rendered text
      // (built in-circuit via string interpolation, see the emission loop
      // below) against the table's recorded rendered text verbatim — no
      // reconstruction of a numeric/composite literal is attempted or
      // needed, so nothing here can be "unparseable" the way the old
      // value-EQ bootstrap could be.
      const expected = renderTextAssert ? wsString(c.output.value) : lit(c.output);
      cases.push({ short, c, expected, renderTextAssert });
    }
  }
  out.push('/// GENERATED by scripts/gen_verifier.mjs — do not edit.');
  out.push(`/// Verifies data/gate_semantics.json (build ${table.build}).`);
  if (skippedUnwired) out.push(`/// skipped ${skippedUnwired} unwired cases (not expressible as assertions).`);
  if (skippedNan) out.push(`/// skipped ${skippedNan} NaN-involved cases (EQ on NaN cannot assert; certified by the probe only).`);
  if (skippedDeferred) out.push(`/// skipped ${skippedDeferred} deferredOps cases (never folded — probed/collected only, see gen_semantics.mjs).`);
  if (skippedTemplate) out.push(`/// skipped ${skippedTemplate} String_FormatText cases (template is a synthetic label, not recoverable from the table).`);
  if (skippedNonCtorInput) out.push(`/// skipped ${skippedNonCtorInput} cases with a non-reconstructable composite operand label (see stderr).`);
  if (skippedBlankRender) out.push(`/// skipped ${skippedBlankRender} blank-render composite cases (rotator/quat/color output is always ""; an assertion would be vacuous — see eval.rs's BLANK_RENDER_REFUSED / transitive-certification comments for how these are validated instead).`);
  out.push('/// Composite-output cases and the compositeOps chapter\'s float-output');
  out.push('/// gates (VecDotProduct/VecMagnitudeSquared/VecDistanceSquared/');
  out.push('/// QuatDotProduct) are asserted by RENDERED-TEXT equality against the');
  out.push('/// table, not value EQ: the table records the game\'s FormatText-rounded');
  out.push('/// (3-decimal) text, which a value comparison against full live');
  out.push('/// precision cannot round-trip in general. Full-precision semantics are');
  out.push('/// certified separately by the Rust replay gate (see');
  out.push('/// crates/wirescript/tests/fold_invariants.rs and');
  out.push('/// lower/fold/eval.rs\'s replay_every_certified_case).');
  out.push('let grid = ReadBrickGrid()');
  out.push('');
  out.push('mod failLine(msg: string) {');
  out.push('  PrintToConsole(msg)');
  out.push('  await SleepTicks(_, delay = 1)');
  out.push('}');
  out.push('');
  out.push('on grid {');
  out.push(`  PrintToConsole("VERIFIER v${table.probeVersion} table ${table.build}")`);
  out.push('  var pass: int = 0');
  out.push(`  var total: int = ${cases.length}`);
  let n = 0;
  for (const { short, c, expected, renderTextAssert } of cases) {
    n++;
    const desc = wsEscape(`${short} ${c.inputs.map(i => `${i.variant}:${i.value}`).join(' ')}`);
    // A composite GOT expression only presents its true (vector/rotator/
    // color/quat) rendered form once wrapped in an outer `Opaque(...)`:
    // `Opaque(x) + Opaque(y)` resolves its STATIC result type via the
    // first-matching numeric overload (Float) even though the gate itself
    // carries a composite-variant wire at runtime (see
    // probes/gate_semantics.ws's mathSSAdd comment for the same phenomenon
    // on strings) — confirmed against a real `just check` run. Scalar cases
    // don't need this, their result types already resolve concretely.
    const isCompositeOut = COMPOSITE_VARIANTS.has(c.output.variant);
    const wrap = gotExpr => isCompositeOut ? `Opaque(${gotExpr})` : gotExpr;
    if (short === 'Branch') {
      out.push(`  var got${n}: string = ""`);
      out.push(`  if Opaque(${lit(c.inputs[0])}) { got${n} = "A" } else { got${n} = "B" }`);
    } else if (short === 'Select') {
      out.push(`  let got${n} = if Opaque(${lit(c.inputs[0])}) then 111 else 222`);
    } else if (short === 'LogicalNOT') {
      out.push(`  let got${n} = !Opaque(${lit(c.inputs[0])})`);
    } else if (BIN_OP[short]) {
      const op = BIN_OP[short];
      out.push(`  let got${n} = ${wrap(`Opaque(${lit(c.inputs[0])}) ${op} Opaque(${lit(c.inputs[1])})`)}`);
    } else if (CALL_FN[short]) {
      const fn = CALL_FN[short];
      const args = c.inputs.map(i => `Opaque(${lit(i)})`).join(', ');
      out.push(`  let got${n} = ${wrap(`${fn}(${args})`)}`);
    } else {
      throw new Error(`no operator/call mapping for ${short}`);
    }
    if (renderTextAssert) {
      // Rendered-text assertion (see usesRenderedTextAssert): bind the live
      // result's OWN rendered text via string interpolation, then compare
      // THAT string against the table's recorded rendered text — this
      // asserts exactly the contract the table records, sidestepping any
      // precision mismatch between the 3-decimal-rounded table text and the
      // live full-precision value.
      out.push(`  let lr${n} = "\${got${n}}"`);
      out.push(`  if lr${n} == ${expected} { pass = pass + 1 } else { failLine("FAIL ${desc} expected ${c.output.value} got \${lr${n}}") }`);
    } else {
      out.push(`  if got${n} == ${expected} { pass = pass + 1 } else { failLine("FAIL ${desc} expected ${c.output.value} got \${got${n}}") }`);
    }
  }
  out.push('  PrintToConsole("VERIFY ${pass}/${total}")');
  out.push('}');
  return out.join('\n') + '\n';
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  const args = process.argv.slice(2);
  const tablePath = args.includes('--table') ? args[args.indexOf('--table') + 1] : DEFAULT_TABLE;
  const outPath = args.includes('--out') ? args[args.indexOf('--out') + 1] : DEFAULT_VERIFIER_OUT;
  const table = JSON.parse(readFileSync(tablePath, 'utf8'));
  const ws = generateVerifier(table);
  writeFileSync(outPath, ws);
  console.log(`wrote ${outPath}`);
}
