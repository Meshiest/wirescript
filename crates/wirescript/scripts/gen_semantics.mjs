#!/usr/bin/env node
// Regenerate data/gate_semantics.json from a pasted probe console dump.
// The ONLY writer of that file — hand-authored entries are not allowed.
//   node scripts/gen_semantics.mjs dump.txt --build cl16xxx [--out data/gate_semantics.json]
import { readFileSync, writeFileSync } from 'fs';
import { pathToFileURL, fileURLToPath } from 'url';
import { dirname, join } from 'path';

// Defaults are anchored to this script's own directory (crates/wirescript/),
// not process.cwd() — the pipeline is documented to be invoked from either
// the bearilog repo root (`node crates/wirescript/scripts/gen_semantics.mjs
// ...`) or from within crates/wirescript itself, and both must resolve to
// the same data/gate_semantics.json.
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const DEFAULT_OUT = join(SCRIPT_DIR, '..', 'data', 'gate_semantics.json');

export const EXPECTED_PROBE_VERSION = 3;

const GATE_CLASS = {
  CompareEqual: 'BrickComponentType_WireGraph_Expr_CompareEqual',
  CompareNotEqual: 'BrickComponentType_WireGraph_Expr_CompareNotEqual',
  LogicalAND: 'BrickComponentType_WireGraph_Expr_LogicalAND',
  LogicalOR: 'BrickComponentType_WireGraph_Expr_LogicalOR',
  LogicalNOT: 'BrickComponentType_WireGraph_Expr_LogicalNOT',
  LogicalXOR: 'BrickComponentType_WireGraph_Expr_LogicalXOR',
  CompareLess: 'BrickComponentType_WireGraph_Expr_CompareLess',
  CompareLessEqual: 'BrickComponentType_WireGraph_Expr_CompareLessOrEqual',
  CompareGreater: 'BrickComponentType_WireGraph_Expr_CompareGreater',
  CompareGreaterEqual: 'BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual',
  Select: 'BrickComponentType_WireGraph_Expr_Select',
  Branch: 'BrickComponentType_WireGraph_Exec_Branch',
  MathAdd: 'BrickComponentType_WireGraph_Expr_MathAdd',
  MathSubtract: 'BrickComponentType_WireGraph_Expr_MathSubtract',
  MathMultiply: 'BrickComponentType_WireGraph_Expr_MathMultiply',
  MathDivide: 'BrickComponentType_WireGraph_Expr_MathDivide',
  MathModulo: 'BrickComponentType_WireGraph_Expr_MathModulo',

  // -- v3: strings chapter (crates/wirescript/src/ir/gate_class.rs) --------
  String_Concatenate: 'BrickComponentType_WireGraph_Expr_String_Concatenate',
  String_FormatText: 'BrickComponentType_WireGraph_Expr_String_FormatText',
  String_Length: 'BrickComponentType_WireGraph_Expr_String_Length',
  String_ToLower: 'BrickComponentType_WireGraph_Expr_String_ToLower',
  String_ToUpper: 'BrickComponentType_WireGraph_Expr_String_ToUpper',
  String_Trim: 'BrickComponentType_WireGraph_Expr_String_Trim',
  String_Contains: 'BrickComponentType_WireGraph_Expr_String_Contains',
  String_StartsWith: 'BrickComponentType_WireGraph_Expr_String_StartsWith',
  String_EndsWith: 'BrickComponentType_WireGraph_Expr_String_EndsWith',
  String_Substring: 'BrickComponentType_WireGraph_Expr_String_Substring',
  String_Find: 'BrickComponentType_WireGraph_Expr_String_Find',
  String_Replace: 'BrickComponentType_WireGraph_Expr_String_Replace',
  String_ParseInt: 'BrickComponentType_WireGraph_Expr_String_ParseInt',
  String_ParseNumber: 'BrickComponentType_WireGraph_Expr_String_ParseNumber',

  // -- v3: compositeOps chapter ---------------------------------------------
  MakeVector: 'BrickComponentType_WireGraph_Expr_MakeVector',
  MakeRotation: 'BrickComponentType_WireGraph_Expr_MakeRotation',
  MakeQuaternion: 'BrickComponentType_WireGraph_Expr_MakeQuaternion',
  MakeColor: 'BrickComponentType_WireGraph_Expr_MakeColor',
  MakeColorSRGB: 'BrickComponentType_WireGraph_Expr_MakeColorSRGB',
  MakeColorHex: 'BrickComponentType_WireGraph_Expr_MakeColorHex',
  ColorToHex: 'BrickComponentType_WireGraph_Expr_ColorToHex',
  VecScale: 'BrickComponentType_WireGraph_Expr_VecScale',
  VecDotProduct: 'BrickComponentType_WireGraph_Expr_VecDotProduct',
  VecCrossProduct: 'BrickComponentType_WireGraph_Expr_VecCrossProduct',
  VecMagnitudeSquared: 'BrickComponentType_WireGraph_Expr_VecMagnitudeSquared',
  VecDistanceSquared: 'BrickComponentType_WireGraph_Expr_VecDistanceSquared',
  RotateVector: 'BrickComponentType_WireGraph_Expr_RotateVector',
  InvertRotation: 'BrickComponentType_WireGraph_Expr_InvertRotation',
  QuatDotProduct: 'BrickComponentType_WireGraph_Expr_QuatDotProduct',

  // -- v3: deferredOps chapter — parsed/collected like any other chapter,
  // but gen_verifier.mjs skips every case here with a tally (never folded).
  VecMagnitude: 'BrickComponentType_WireGraph_Expr_VecMagnitude',
  VecNormalize: 'BrickComponentType_WireGraph_Expr_VecNormalize',
  VecDistance: 'BrickComponentType_WireGraph_Expr_VecDistance',
  QuatSlerp: 'BrickComponentType_WireGraph_Expr_QuatSlerp',
  QuatFromAxisAngle: 'BrickComponentType_WireGraph_Expr_QuatFromAxisAngle',
  QuatAngleBetween: 'BrickComponentType_WireGraph_Expr_QuatAngleBetween',
  QuatBetween: 'BrickComponentType_WireGraph_Expr_QuatBetween',
  DirectionToRotation: 'BrickComponentType_WireGraph_Expr_DirectionToRotation',
  RotationToDirection: 'BrickComponentType_WireGraph_Expr_RotationToDirection',
  ColorBlend: 'BrickComponentType_WireGraph_Expr_ColorBlend',
};

// FormatText renders bools as 1/0 and large ints with thousands separators —
// normalize both so the table (and the verifier generated from it) carry
// canonical literals.
function normValue(variant, value) {
  if (variant === 'int' && /^-?[\d,]+$/.test(value)) return value.replace(/,/g, '');
  if (variant === 'bool') {
    if (value === '1') return 'true';
    if (value === '0') return 'false';
  }
  return value;
}

// v3: string operands whose actual value may contain spaces are printed
// quote-delimited by the probe (`str:'  x  '`) instead of bare
// (`str:1` in v2's eqIS-style cases — no probed value there has a space).
// SINGLE quotes are the delimiter — they need no escaping inside the
// probe's own double-quoted wirescript templates, keeping the probe source
// and the log free of `\"` noise. No probed value contains a single quote
// or backslash; a value that did would break simple quote-parity dequoting,
// so treat that as a hard error rather than silently mis-parsing — the
// parser needs an escape rule first.
function dequote(tok, raw) {
  if (raw.startsWith('"')) {
    throw new Error(
      `double-quote-delimited string operand: ${tok} — stale dump format (the probe ` +
      `now single-quote-delimits, str:'...'); re-run the probe.`);
  }
  if (!raw.startsWith("'")) return raw; // bare (v2-style) string operand
  if (raw.length < 2 || !raw.endsWith("'")) {
    throw new Error(
      `unterminated quoted string operand: ${tok} — a probed value likely contains ` +
      `a raw quote/backslash; the parser needs an escape rule before it can handle this.`);
  }
  const inner = raw.slice(1, -1);
  if (inner.includes("'") || inner.includes('\\')) {
    throw new Error(
      `quoted string operand contains a quote or backslash: ${tok} — the parser ` +
      `needs an escape rule before it can handle this.`);
  }
  return inner;
}

function parseOperand(tok) {
  if (tok === 'unwired') return 'unwired';
  if (tok === '-') return null;
  const i = tok.indexOf(':');
  if (i < 0) throw new Error(`bad operand: ${tok}`);
  const variant = tok.slice(0, i);
  let value = tok.slice(i + 1);
  if (variant === 'str') value = dequote(tok, value);
  return { variant, value: normValue(variant, value) };
}

// v3 fix wave: gate short names whose RESULT is always a string. The game's
// log channel strips trailing whitespace per line, which silently corrupted
// values like ToLower("  x  ") -> "  x" before this fix — the probe
// (probes/gate_semantics.ws) now prints every one of these results
// quote-delimited (`-> '...'`, single quotes — no escaping needed inside
// the probe's own double-quoted templates) so the closing quote, not a
// space, is the last character on the line. A CASE line for one of these
// gates arriving WITHOUT quotes is therefore a mixed-format dump (captured
// with the pre-fix probe, before the closing-quote guard existed) — hard
// error rather than silently accepting a value that may have lost trailing
// whitespace in transit. `render`'s `str:` labels get the same treatment
// (handled separately below — the render table isn't keyed by gate short
// name). Every other gate/label (numeric, bool, composite) stays bare, as
// before.
const STRING_RESULT_SHORT_NAMES = new Set([
  'String_Concatenate', 'String_ToLower', 'String_ToUpper', 'String_Trim',
  'String_Substring', 'String_Replace', 'String_FormatText',
]);

// Quotes around a recorded RESULT are transport armor, not value content:
// strip them and keep everything inside verbatim — including leading/
// trailing spaces, the entire point of the fix — same quote-parity/escape-
// rule contract as dequote() above, applied to a CASE line's `-> ...`
// segment instead of an operand.
function dequoteOutput(tok, raw) {
  if (raw.length < 2 || !raw.startsWith("'") || !raw.endsWith("'")) {
    throw new Error(
      `unterminated quoted output for ${tok}: ${JSON.stringify(raw)} — a probed ` +
      `value likely contains a raw quote/backslash; the parser needs an escape rule ` +
      `before it can handle this.`);
  }
  const inner = raw.slice(1, -1);
  if (inner.includes("'") || inner.includes('\\')) {
    throw new Error(
      `quoted output contains a quote or backslash for ${tok}: ${JSON.stringify(raw)} ` +
      `— the parser needs an escape rule before it can handle this.`);
  }
  return inner;
}

// Gates whose output port is bool — their outputs print as 1/0 through
// FormatText and must not be classified int.
const BOOL_OUTPUT = new Set([
  'CompareEqual', 'CompareNotEqual', 'CompareLess', 'CompareLessEqual',
  'CompareGreater', 'CompareGreaterEqual',
  'LogicalAND', 'LogicalOR', 'LogicalNOT', 'LogicalXOR',
  // v3 strings chapter — Contains/StartsWith/EndsWith are bool-output gates.
  'String_Contains', 'String_StartsWith', 'String_EndsWith',
]);

// v3: the four composite (non-scalar) wire variants. Their rendered text is
// never interpreted here (that's Task 3's job, against certified render
// laws) — it is stored verbatim as `value`.
const COMPOSITE_VARIANTS = new Set(['vector', 'rotator', 'color', 'quat']);

// Gates whose output is ALWAYS one of the four composite variants,
// regardless of operand types (constructors / conversions / vector-rotation
// ops). MathAdd/Subtract/Multiply/Divide/Modulo are handled separately below
// since they're shared between the scalar `math` chapter and the vector
// `compositeMath` chapter.
const COMPOSITE_OUTPUT_GATE = {
  MakeVector: 'vector', MakeRotation: 'rotator', MakeQuaternion: 'quat',
  MakeColor: 'color', MakeColorSRGB: 'color', MakeColorHex: 'color',
  VecScale: 'vector', RotateVector: 'vector', InvertRotation: 'quat',
  VecCrossProduct: 'vector', VecNormalize: 'vector', QuatSlerp: 'quat',
  QuatFromAxisAngle: 'quat', QuatBetween: 'quat',
  DirectionToRotation: 'rotator', RotationToDirection: 'vector',
  ColorBlend: 'color',
};

// Math gates shared between the scalar `math` chapter and the vector
// `compositeMath` chapter — the ONLY gates where a composite operand implies
// a composite (same-variant) output. Gates like CompareEqual, VecDotProduct,
// or VecMagnitude also take composite operands but do NOT share this
// pass-through (their output stays bool/float) — this must not be a blanket
// "any composite input -> composite output" rule.
const SHARED_COMPOSITE_MATH_GATES = new Set([
  'MathAdd', 'MathSubtract', 'MathMultiply', 'MathDivide', 'MathModulo',
]);

function outVariant(raw, gateShort, inputs) {
  if (COMPOSITE_OUTPUT_GATE[gateShort]) {
    return { variant: COMPOSITE_OUTPUT_GATE[gateShort], value: raw };
  }
  if (SHARED_COMPOSITE_MATH_GATES.has(gateShort)) {
    const compositeIn = (inputs ?? []).find(i => i && i !== 'unwired' && COMPOSITE_VARIANTS.has(i.variant));
    if (compositeIn) {
      return { variant: compositeIn.variant, value: raw };
    }
  }
  if (BOOL_OUTPUT.has(gateShort)) {
    if (raw === '1' || raw === 'true') return { variant: 'bool', value: 'true' };
    if (raw === '0' || raw === 'false') return { variant: 'bool', value: 'false' };
    throw new Error(`unexpected bool-gate output token: ${raw} (${gateShort})`);
  }
  if (raw === 'true' || raw === 'false') return { variant: 'bool', value: raw };
  if (/^-?[\d,]+$/.test(raw)) return { variant: 'int', value: raw.replace(/,/g, '') };
  if (/^-?(\d+\.\d*|nan|inf|-inf)$/i.test(raw)) return { variant: 'float', value: raw };
  return { variant: 'str', value: raw };
}

// Probe-line markers. Raw game-log dumps prefix every line with
// `[timestamp][frame]LogBrickadia: ...` — keep only from the first marker
// on and DROP non-probe lines entirely (log noise between probe lines is
// expected; dropped CASE lines are still caught by chapter-count checks).
const MARKER_RE = /(PROBE |BEGIN |END |CASE ).*$/;

// v3: a CASE line's operand count varies per gate (1 for unary string
// builtins, up to 4 for MakeColor/MakeQuaternion), and string operands may
// be quote-delimited with spaces inside (`str:'  x  '` — SINGLE quotes, see
// dequote above), so a fixed-arity regex (v2's CASE_RE) no longer works.
// Tokenize by whitespace, but keep merging across whitespace while inside
// an open quote or paren — this keeps `str:'  x  '` and
// `vector:Vec(0.5, -1.25, 1.0/3.0)`-shaped tokens (space after a comma)
// intact as one token. The `->` separator always stands alone between
// whitespace, so it's found the same way once the tokenizer no longer
// misfires on the operands around it.
function tokenizeCaseRest(rest) {
  const tokens = [];
  let i = 0;
  const n = rest.length;
  while (i < n) {
    while (i < n && /\s/.test(rest[i])) i++;
    if (i >= n) break;
    const start = i;
    let quoteOpen = false;
    let parenDepth = 0;
    while (i < n) {
      const ch = rest[i];
      if (ch === "'") { quoteOpen = !quoteOpen; i++; continue; }
      if (ch === '(') { parenDepth++; i++; continue; }
      if (ch === ')') { if (parenDepth > 0) parenDepth--; i++; continue; }
      if (!quoteOpen && parenDepth === 0 && /\s/.test(ch)) break;
      i++;
    }
    if (quoteOpen) {
      throw new Error(
        `unterminated quote in operand token starting at "${rest.slice(start, Math.min(start + 40, n))}" ` +
        `— a probed value likely contains a raw quote/backslash; the parser needs an escape rule before it can handle this.`);
    }
    tokens.push({ text: rest.slice(start, i), end: i });
  }
  return tokens;
}

function parseCaseLine(line) {
  const rest = line.slice('CASE '.length);
  const tokens = tokenizeCaseRest(rest);
  if (tokens.length < 2) throw new Error(`malformed CASE line: ${line}`);
  const gateShort = tokens[0].text;
  const arrowIdx = tokens.findIndex((t, idx) => idx > 0 && t.text === '->');
  if (arrowIdx < 0) throw new Error(`CASE line missing '->' separator: ${line}`);
  const operandTokens = tokens.slice(1, arrowIdx).map(t => t.text);
  let output = rest.slice(tokens[arrowIdx].end);
  if (output.startsWith(' ')) output = output.slice(1);
  return { gateShort, operandTokens, output };
}

export function parseDump(text, expectedVersion) {
  const lines = text
    .split(/\r?\n/)
    .map(l => l.match(MARKER_RE)?.[0].trim())
    .filter(Boolean);
  const header = lines[0]?.match(/^PROBE gate_semantics v(\d+)$/);
  if (!header) throw new Error('missing PROBE header');
  if (Number(header[1]) !== expectedVersion) {
    throw new Error(`probe version ${header[1]} != expected ${expectedVersion} — stale probe pasted?`);
  }
  const gates = {};
  // v3: `render` is a top-level table section (label -> raw rendered text,
  // verbatim, commas and all), NOT a gate — it calibrates how every other
  // chapter's composite/numeric renders should be read back (Task 3's job).
  const render = {};
  let chapter = null; // { name, declared, seen }
  for (const line of lines.slice(1)) {
    let m;
    if ((m = line.match(/^BEGIN (\S+) (\d+)$/))) {
      if (chapter) throw new Error(`BEGIN ${m[1]} inside chapter ${chapter.name}`);
      chapter = { name: m[1], declared: Number(m[2]), seen: 0 };
    } else if ((m = line.match(/^END (\S+)$/))) {
      if (!chapter || chapter.name !== m[1]) throw new Error(`unmatched END ${m[1]}`);
      if (chapter.seen !== chapter.declared) {
        throw new Error(`chapter ${chapter.name}: declared ${chapter.declared} cases, saw ${chapter.seen} — dropped lines?`);
      }
      chapter = null;
    } else if (line.startsWith('CASE ')) {
      if (!chapter) throw new Error(`CASE outside chapter: ${line}`);
      chapter.seen++;
      const { gateShort, operandTokens, output } = parseCaseLine(line);
      if (gateShort === 'Render') {
        if (operandTokens.length !== 1) {
          throw new Error(`Render CASE must have exactly one label operand: ${line}`);
        }
        const label = operandTokens[0];
        if (Object.prototype.hasOwnProperty.call(render, label)) {
          throw new Error(`duplicate render label: ${label}`);
        }
        // v3 fix wave: the render chapter's string-variant case (label
        // "str:...") is the other spot the probe now quote-delimits its
        // result (see STRING_RESULT_SHORT_NAMES above) — every other render
        // label (int/float/bool/vector/rotator/color/quat) stays bare.
        if (label.startsWith('str:')) {
          if (!output.startsWith("'")) {
            throw new Error(
              `render label ${label}: string render result must be quote-delimited ` +
              `(-> '...') — got unquoted ${JSON.stringify(output)}; stale/mixed-format dump?`);
          }
          render[label] = dequoteOutput(`Render ${label}`, output);
        } else {
          render[label] = output;
        }
      } else {
        const cls = GATE_CLASS[gateShort];
        if (!cls) throw new Error(`unknown gate short name: ${gateShort}`);
        const inputs = operandTokens.map(parseOperand).filter(x => x !== null);
        const entry = (gates[cls] ??= { cases: [], rules: [] });
        let outputVal;
        if (STRING_RESULT_SHORT_NAMES.has(gateShort)) {
          if (!output.startsWith("'")) {
            throw new Error(
              `${gateShort}: string-result CASE must be quote-delimited (-> '...') ` +
              `— got unquoted ${JSON.stringify(output)}; stale/mixed-format dump?`);
          }
          outputVal = { variant: 'str', value: dequoteOutput(gateShort, output) };
        } else {
          outputVal = outVariant(output, gateShort, inputs);
        }
        entry.cases.push({ inputs, output: outputVal });
      }
    } else if (line === 'PROBE done') {
      if (chapter) throw new Error(`dump ended inside chapter ${chapter.name}`);
    } else {
      throw new Error(`unrecognized line: ${line}`);
    }
  }
  return { probeVersion: expectedVersion, gates, render };
}

export function deriveRules(gateEntry, gateClass) {
  // Annihilator rules are NOT induced open-endedly from the observations —
  // with the probe's small reused value pool, "same output across >=2
  // partners" is trivially satisfied by coincidence (e.g. `"a" > ""` and
  // `"a" > int` both false does not make `> "a"` an algebraic law). Instead,
  // a fixed whitelist of candidate laws is CONFIRMED against the cases:
  // emitted only when every observed case involving the operand agrees with
  // the law across >= MIN_PARTNERS distinct partners; a contradicting
  // observation is a hard error (game semantics disagree with the law).
  const CANDIDATES = {
    'BrickComponentType_WireGraph_Expr_LogicalAND': [
      { when: { variant: 'bool', value: 'false' }, result: { variant: 'bool', value: 'false' } },
    ],
    'BrickComponentType_WireGraph_Expr_LogicalOR': [
      { when: { variant: 'bool', value: 'true' }, result: { variant: 'bool', value: 'true' } },
    ],
  };
  const MIN_PARTNERS = 2;
  const rules = [];
  for (const cand of CANDIDATES[gateClass] ?? []) {
    const partners = new Set();
    let contradicted = null;
    for (const c of gateEntry.cases) {
      for (const [i, inp] of c.inputs.entries()) {
        if (inp === 'unwired' || !inp) continue;
        if (inp.variant !== cand.when.variant || inp.value !== cand.when.value) continue;
        const partner = c.inputs[1 - i];
        if (partner === undefined) continue; // unary case — not this law
        if (c.output.variant === cand.result.variant && c.output.value === cand.result.value) {
          partners.add(partner === 'unwired' ? 'unwired' : `${partner.variant}:${partner.value}`);
        } else {
          contradicted = c;
        }
      }
    }
    if (contradicted) {
      throw new Error(
        `annihilator candidate ${cand.when.variant}:${cand.when.value} for ${gateClass} ` +
        `contradicted by observed case -> ${contradicted.output.variant}:${contradicted.output.value}`);
    }
    if (partners.size >= MIN_PARTNERS) {
      rules.push({ kind: 'annihilator', when: cand.when, result: cand.result });
    }
  }
  return rules;
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  const args = process.argv.slice(2);
  const dumpPath = args.find(a => !a.startsWith('--'));
  const build = args[args.indexOf('--build') + 1];
  const out = args.includes('--out') ? args[args.indexOf('--out') + 1] : DEFAULT_OUT;
  if (!dumpPath || !build || build.startsWith('--')) {
    console.error('usage: node scripts/gen_semantics.mjs <dump.txt> --build <label> [--out <path>]');
    process.exit(1);
  }
  const table = parseDump(readFileSync(dumpPath, 'utf8'), EXPECTED_PROBE_VERSION);
  for (const [cls, entry] of Object.entries(table.gates)) entry.rules = deriveRules(entry, cls);
  const doc = { build, generatedAt: new Date().toISOString(), ...table };
  writeFileSync(out, JSON.stringify(doc, null, 2) + '\n');
  const nCases = Object.values(table.gates).reduce((a, g) => a + g.cases.length, 0);
  const nRender = Object.keys(table.render ?? {}).length;
  console.log(`wrote ${out}: ${Object.keys(table.gates).length} gates, ${nCases} cases, ${nRender} render entries`);
}
