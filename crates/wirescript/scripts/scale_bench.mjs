#!/usr/bin/env node
// Scaling benchmark: generate each shape below at 4k, 8k and 16k
// declarations, time every compile stage with the release `profile_stages`
// example, and report how each stage grows from 8k to 16k. A linear stage
// grows about 2x per doubling and a quadratic one about 4x, so the gate fails
// any stage whose 8k -> 16k ratio is above 2.6.
//
//   node crates/wirescript/scripts/scale_bench.mjs            # table + gate
//   node crates/wirescript/scripts/scale_bench.mjs --update   # also write the baseline
//   node crates/wirescript/scripts/scale_bench.mjs --shapes vars,chips --runs 5
//
// Each size runs `--runs` times (3 by default) and keeps each stage's fastest
// time. A stage under the noise floor at 16k is shown but not judged: a few
// milliseconds doubling or tripling is timer noise, not growth. A stage over
// the limit is timed again at twice the largest size, and again at twice
// that while it stays over, and judged on the last doubling: a working set
// outgrowing a cache or a noisy run fades with size, while a quadratic stage
// grows about 4x at every doubling.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { binPath, opt, repoRoot } from './lib.mjs';

const SIZES = [4000, 8000, 16000];
const LIMIT = 2.6;
const NOISE_FLOOR_MS = 10;
// One run's limit; a run past it fails its shape as too slow.
const RUN_TIMEOUT_MS = 120_000;
// A stage this slow at the largest size is not timed again larger: whatever
// its ratio, the next size would cost minutes and gigabytes.
const CONFIRM_MAX_MS = 20_000;
const STAGES = ['resolve', 'typecheck', 'lower', 'layout', 'build_world'];

const R = (k) => Array.from({ length: k }, (_, i) => i);
const SHAPES = {
  // N vars, one handler writing each once
  vars: (n) => ['in start: exec', ...R(n).map((i) => `var v${i}: int = 0`), 'on start {', ...R(n).map((i) => `  v${i} = v${i} + ${i}`), '}'],
  // one var, N writes in one handler (long exec chain)
  chain: (n) => ['in start: exec', 'var x: int = 0', 'on start {', ...R(n).map((i) => `  x = x + ${i}`), '}'],
  // N chained pure lets
  lets: (n) => ['in a: int', 'let l0 = a + 1', ...R(n).slice(1).map((i) => `let l${i} = l${i - 1} * 3 + ${i}`), `out o: int = l${n - 1}`],
  // N exec inputs, each with its own handler, all writing one var
  handlers: (n) => ['var x: int = 0', ...R(n).flatMap((i) => [`in s${i}: exec`, `on s${i} { x = x + ${i} }`])],
  // N if statements in one handler
  ifs: (n) => ['in start: exec', 'in c: int', 'var x: int = 0', 'on start {', ...R(n).map((i) => `  if c > ${i} { x = x + ${i} }`), '}'],
  // one mod inlined N times
  calls: (n) => ['in start: exec', 'var x: int = 0', 'mod inc(v: *int, by: int) { v = v + by }', 'on start {', ...R(n).map((i) => `  inc(x, ${i})`), '}'],
  // N chips, each with a var and a handler
  chips: (n) => ['in start: exec', ...R(n).map((i) => `chip { var c${i}: int = 0\n  on start { c${i} = c${i} + 1 } }`)],
  // N unused vars and N writes to one var: isolates writes x bindings
  decl_writes: (n) => ['in start: exec', ...R(n).map((i) => `var v${i}: int = 0`), 'on start {', ...R(n).map((i) => `  v0 = v0 + ${i}`), '}'],
  // N unused pure lets and N writes to one var
  lets_writes: (n) => ['in start: exec', 'in a: int', 'var x: int = 0', ...R(n).map((i) => `let l${i} = a + ${i}`), 'on start {', ...R(n).map((i) => `  x = x + ${i}`), '}'],
  // the vars shape under the two non-default layouts (the blank line after @layout is required)
  code: (n) => ['@layout("code")', '', ...SHAPES.vars(n)],
  cube: (n) => ['@layout("cube")', '', ...SHAPES.vars(n)],
  // code layout calling an imported mod of N statements once: the body's nodes come from another file
  code_import: () => ['@layout("code")', '', 'import { big } from "code_lib"', 'in start: exec', 'on start { big() }'],
  // code layout with a comment above each of N statements
  code_comments: (n) => ['@layout("code")', '', 'in start: exec', 'var x: int = 0', 'on start {', ...R(n).flatMap((i) => [`  // step ${i}`, `  x = x * 3 + ${i}`]), '}'],
  // code layout with one var read on N lines (one value bused to every line)
  code_reads: (n) => ['@layout("code")', '', 'in start: exec', 'var x: int = 0', 'var y: int = 0', 'on start {', ...R(n).map((i) => `  y = x + ${i}`), '}'],
  // a mod declaring a local exec signal, inlined N times (one exec hub per call)
  hubs: (n) => ['in start: exec', 'var x: int = 0', 'mod m(v: *int) {', '  let done: exec', '  emit done', '  await done', '  v = v + 1', '}', 'on start {', ...R(n).map(() => '  m(x)'), '}'],
  // one const array read at run time (so it gets its gate) and N storing writes
  const_writes: (n) => ['in start: exec', 'const xs = [10, 20, 30]', 'var x: int = 0', 'on start {', '  x = xs[x % 3]', ...R(n).map((i) => `  x = x * 3 + ${i}`), '}'],
  // one const array read at run time and N named-chip instances (each checks its pins)
  const_chips: (n) => ['in start: exec', 'const xs = [10, 20, 30]', 'var x: int = 0', 'chip C(v: int) -> (o: int) { out o = v + 1 }', 'on start {', '  x = xs[x % 3]', ...R(n).map(() => '  x = C(x)'), '}'],
  // a value-returning mod inlined N times
  calls_ret: (n) => ['in start: exec', 'var x: int = 0', 'mod f(a: int) -> int { return a + 1 }', 'on start {', ...R(n).map(() => '  x = f(x)'), '}'],
  // N calls into an `import * as` module of 100 mods (written beside the program)
  ns_calls: (n) => ['import * as L from "ns_lib"', 'in start: exec', 'var x: int = 0', 'on start {', ...R(n).map((i) => `  L.f${i % 100}(x)`), '}'],
  // N top-level vars, each shadowed by a var in one handler (N locals in one frame)
  shadow: (n) => ['in start: exec', ...R(n).map((i) => `var t${i}: int = 0`), 'on start {', ...R(n).flatMap((i) => [`  var t${i}: int = ${i}`, `  t${i} = t${i} + 1`]), '}'],
  // N top-level vars, each shadowed by a var in one inlined mod body
  mod_shadow: (n) => ['in start: exec', ...R(n).map((i) => `var t${i}: int = 0`), 'mod m() {', ...R(n).flatMap((i) => [`  var t${i}: int = ${i}`, `  t${i} = t${i} + 1`]), '}', 'on start { m() }'],
  // N functions each writing one outer var through a baked handle, each called once
  fns: (n) => ['in start: exec', 'var g: int = 0', ...R(n).map((i) => `fn f${i}(x: int) { g += x }`), 'on start {', ...R(n).map((i) => `  f${i}(${i})`), '}'],
  // N small buffered back-edge loops in one module, at sizes the DAG layout takes (at most 5,000 nodes)
  dag_loops: (n) => ['in a: int', ...R(n).map((i) => `buffer b${i}: int = b${i} + a`), `out o: int = b0 + b${n - 1}`],
  // a chain of N mods, each calling the one before, called once
  mod_chain: (n) => ['in start: exec', 'var x: int = 0', 'mod m0(v: *int) { v = v + 1 }', ...R(n).slice(1).map((i) => `mod m${i}(v: *int) { m${i - 1}(v) }`), `on start { m${Math.min(n - 1, 40)}(x) }`],
  // a chain of N mods whose last writes an outer var: the use travels up every link (WS091)
  use_chain: (n) => ['in start: exec', 'var g: int = 0', 'mod m0() { g = g + 1 }', ...R(n).slice(1).map((i) => `mod m${i}() { m${i - 1}() }`), `on start { m${Math.min(n - 1, 40)}() }`],
  // N named imports, each from its own library declaring a type alias
  alias_imports: (n) => [...R(n).map((i) => `import { f${i} } from "alias_lib${i}"`), 'in start: exec', 'var x: int = 0', 'on start {', '  f0(x)', '}'],
  // N lines, each with an undefined name
  errors: (n) => ['in start: exec', ...R(n).map((i) => `let e${i} = missing${i} + 1`)],
  // a chain of N selects on a condition only the fold pass decides (a chip's constant output)
  sc_chain: (n) => ['in a: int', 'chip K(c: bool) -> (o: bool) { out o = c }', 'let c = K(true)', 'let s0 = a', ...R(n).slice(1).map((i) => `let s${i} = if c then s${i - 1} else ${i}`), `out o: int = s${n - 1}`],
};

// Sizes for a shape the default sizes do not suit.
const SHAPE_SIZES = {
  dag_loops: [400, 800, 1600],
  alias_imports: [500, 1000, 2000],
};

// Files a shape imports, written beside it.
const SUPPORT = {
  ns_calls: () => ({ 'ns_lib.ws': R(100).map((i) => `mod f${i}(v: *int) { v = v + ${i} }`) }),
  alias_imports: (n) => Object.fromEntries(R(n).map((i) => [`alias_lib${i}.ws`, [`type T${i} = int`, `mod f${i}(v: *T${i}) { v = v + ${i} }`]])),
  code_import: (n) => ({ 'code_lib.ws': ['mod big() {', '  var t: int = 0', ...R(n).map((i) => `  t = t * 3 + ${i}`), '}'] }),
};

/** A `profile_stages` duration (`12.34ms`, `1.20s`, `850.00µs`) in milliseconds. */
function millis(text) {
  const m = text.trim().match(/^([\d.]+)\s*(ns|µs|us|ms|s)$/);
  if (!m) throw new Error(`scale_bench: unreadable duration ${JSON.stringify(text)}`);
  const scale = { ns: 1e-6, µs: 1e-3, us: 1e-3, ms: 1, s: 1000 }[m[2]];
  return Number(m[1]) * scale;
}

/** What `profile_stages` prints on `file`: it reports on stderr. */
function runOnce(bin, file) {
  const r = spawnSync(bin, [file], { encoding: 'utf8', maxBuffer: 1 << 26, timeout: RUN_TIMEOUT_MS });
  if (r.error?.code === 'ETIMEDOUT') throw new Error(`scale_bench: profile_stages took over ${RUN_TIMEOUT_MS / 1000}s on ${file}`);
  if (r.status !== 0) throw new Error(`scale_bench: profile_stages failed on ${file}:
${r.stderr ?? r.error}`);
  return r.stderr;
}

/** Each stage's time, in milliseconds, for one run on `file`. */
function stageTimes(bin, file) {
  const text = runOnce(bin, file);
  const times = {};
  for (const stage of STAGES) {
    const m = text.match(new RegExp(`^${stage}[^:]*:\\s+(\\S+)`, 'm'));
    if (!m) throw new Error(`scale_bench: no "${stage}" line from profile_stages on ${file}`);
    times[stage] = millis(m[1]);
  }
  return times;
}

function main() {
  const args = process.argv.slice(2);
  const update = args.includes('--update');
  const runs = Number(opt('--runs', '3'));
  const wanted = opt('--shapes', Object.keys(SHAPES).join(',')).split(',');
  for (const s of wanted) if (!SHAPES[s]) throw new Error(`scale_bench: no shape "${s}"`);

  execFileSync('cargo', ['build', '--release', '-p', 'wirescript', '--example', 'profile_stages'], {
    cwd: repoRoot,
    stdio: ['ignore', 'ignore', 'inherit'],
  });
  const bin = binPath(join('examples', 'profile_stages'));
  const dir = join(repoRoot, 'target', 'scale-bench');
  mkdirSync(dir, { recursive: true });

  /** Each stage's fastest time over `runs` runs of `shape` at size `n`. */
  const measure = (shape, n) => {
    const file = join(dir, `${shape}_${n}.ws`);
    writeFileSync(file, SHAPES[shape](n).join('\n') + '\n');
    for (const [name, lines] of Object.entries(SUPPORT[shape]?.(n) ?? {})) writeFileSync(join(dir, name), lines.join('\n') + '\n');
    const best = Object.fromEntries(STAGES.map((s) => [s, Infinity]));
    for (let r = 0; r < runs; r++) {
      const t = stageTimes(bin, file);
      for (const s of STAGES) best[s] = Math.min(best[s], t[s]);
    }
    return best;
  };
  const results = {};
  for (const shape of wanted) {
    results[shape] = {};
    for (const n of SHAPE_SIZES[shape] ?? SIZES) results[shape][n] = measure(shape, n);
  }

  const baselinePath = join(repoRoot, 'crates', 'wirescript', 'scripts', 'scale_bench.baseline.json');
  const baseline = existsSync(baselinePath) ? JSON.parse(readFileSync(baselinePath, 'utf8')) : null;
  const fmt = (ms) => (ms >= 100 ? ms.toFixed(0) : ms.toFixed(1));
  const failures = [];
  console.log(`stage times in ms at ${SIZES.join(' / ')}, ratio = t(16k) / t(8k); fail above ${LIMIT}`);
  console.log(`${'shape'.padEnd(12)} ${'stage'.padEnd(12)} ${'times'.padEnd(26)} ratio   vs baseline (16k)`);
  for (const shape of wanted) {
    for (const stage of STAGES) {
      const sizes = SHAPE_SIZES[shape] ?? SIZES;
      const [a, b, c] = sizes.map((n) => results[shape][n][stage]);
      const ratio = c / b;
      const judged = c >= NOISE_FLOOR_MS;
      let bad = judged && ratio > LIMIT;
      let confirm = '';
      for (let n = sizes[2] * 2, prev = c; bad && prev < CONFIRM_MAX_MS && n <= sizes[2] * 4; n *= 2) {
        results[shape][n] ??= measure(shape, n);
        const t = results[shape][n][stage];
        bad = t / prev > LIMIT;
        confirm += `  (at ${n}: ${fmt(t)}, ratio ${(t / prev).toFixed(2)})`;
        prev = t;
      }
      if (bad) failures.push(`${shape}/${stage} ${ratio.toFixed(2)}`);
      const base = baseline?.[shape]?.[sizes[2]]?.[stage];
      const delta = base ? `${fmt(base)} -> ${fmt(c)}` : '';
      const mark = (bad ? '  FAIL' : judged ? '' : '  (under noise floor)') + confirm;
      console.log(
        `${shape.padEnd(12)} ${stage.padEnd(12)} ${[a, b, c].map(fmt).join(' / ').padEnd(26)} ${ratio.toFixed(2).padStart(5)}   ${delta}${mark}`,
      );
    }
  }

  if (update) {
    const merged = { ...(baseline ?? {}), ...results };
    writeFileSync(baselinePath, JSON.stringify(merged, null, 2) + '\n');
    console.log(`scale_bench: wrote ${baselinePath}`);
    return;
  }
  if (failures.length) {
    console.error(`scale_bench: ${failures.length} stage(s) grow faster than ${LIMIT}x per doubling: ${failures.join(', ')}`);
    process.exit(1);
  }
  console.log('scale_bench: every stage scales within the limit');
}

main();
