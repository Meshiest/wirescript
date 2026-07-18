#!/usr/bin/env node
// Regenerate data/gate_semantics.json from a pasted probe console dump.
// The ONLY writer of that file — hand-authored entries are not allowed.
//   node scripts/gen_semantics.mjs dump.txt --build cl16xxx [--out data/gate_semantics.json]
import { readFileSync, writeFileSync } from 'fs';
import { pathToFileURL } from 'url';

export const EXPECTED_PROBE_VERSION = 2;

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
};

const CASE_RE = /^CASE (\S+) (\S+) (\S+) -> (.*)$/;

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

function parseOperand(tok) {
  if (tok === 'unwired') return 'unwired';
  if (tok === '-') return null;
  const i = tok.indexOf(':');
  if (i < 0) throw new Error(`bad operand: ${tok}`);
  const variant = tok.slice(0, i);
  return { variant, value: normValue(variant, tok.slice(i + 1)) };
}

// Gates whose output port is bool — their outputs print as 1/0 through
// FormatText and must not be classified int.
const BOOL_OUTPUT = new Set([
  'CompareEqual', 'CompareNotEqual', 'CompareLess', 'CompareLessEqual',
  'CompareGreater', 'CompareGreaterEqual',
  'LogicalAND', 'LogicalOR', 'LogicalNOT', 'LogicalXOR',
]);

function outVariant(raw, gateShort) {
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
    } else if ((m = line.match(CASE_RE))) {
      if (!chapter) throw new Error(`CASE outside chapter: ${line}`);
      chapter.seen++;
      const cls = GATE_CLASS[m[1]];
      if (!cls) throw new Error(`unknown gate short name: ${m[1]}`);
      const inputs = [parseOperand(m[2]), parseOperand(m[3])].filter(x => x !== null);
      const entry = (gates[cls] ??= { cases: [], rules: [] });
      entry.cases.push({ inputs, output: outVariant(m[4], m[1]) });
    } else if (line === 'PROBE done') {
      if (chapter) throw new Error(`dump ended inside chapter ${chapter.name}`);
    } else {
      throw new Error(`unrecognized line: ${line}`);
    }
  }
  return { probeVersion: expectedVersion, gates };
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
  const out = args.includes('--out') ? args[args.indexOf('--out') + 1] : 'data/gate_semantics.json';
  if (!dumpPath || !build || build.startsWith('--')) {
    console.error('usage: node scripts/gen_semantics.mjs <dump.txt> --build <label> [--out <path>]');
    process.exit(1);
  }
  const table = parseDump(readFileSync(dumpPath, 'utf8'), EXPECTED_PROBE_VERSION);
  for (const [cls, entry] of Object.entries(table.gates)) entry.rules = deriveRules(entry, cls);
  const doc = { build, generatedAt: new Date().toISOString(), ...table };
  writeFileSync(out, JSON.stringify(doc, null, 2) + '\n');
  const nCases = Object.values(table.gates).reduce((a, g) => a + g.cases.length, 0);
  console.log(`wrote ${out}: ${Object.keys(table.gates).length} gates, ${nCases} cases`);
}
