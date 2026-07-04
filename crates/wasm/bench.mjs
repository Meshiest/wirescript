#!/usr/bin/env node
// Benchmark: compare baseline vs optimized WASM builds
// Usage: node bench.mjs

import { readFileSync } from 'fs';
import { performance } from 'perf_hooks';

// ── Load both WASM modules ──────────────────────────────────────────

async function loadWasm(dir) {
  const wasmBytes = readFileSync(`${dir}/wasm_bg.wasm`);
  const js = await import(`${dir}/wasm.js`);
  js.initSync({ module: wasmBytes });
  return js;
}

const baseline = await loadWasm('./pkg-baseline');
const optimized = await loadWasm('./pkg');

// ── Test programs ───────────────────────────────────────────────────

const SMALL_PROGRAM = `var x: int = 0
var y: int = 0
on Bumped {
  x = x + 1
  y = y + x
}
out result = x + y`;

const MEDIUM_PROGRAM = `var score: int = 0
var health: int = 100
var lives: int = 3
var ammo: int = 50
var combo: int = 0

on Bumped {
  score = score + 10 * combo
  combo = combo + 1
  if health < 50 then {
    ammo = ammo + 5
  } else {
    ammo = ammo + 1
  }
}

on RoundStart {
  score = 0
  health = 100
  lives = 3
  ammo = 50
  combo = 0
}

on CharacterDied {
  lives = lives - 1
  health = 100
  combo = 0
}

out displayScore = score
out displayHealth = health
out displayLives = lives
out displayAmmo = ammo`;

const LARGE_PROGRAM = generateLargeProgram();

function generateLargeProgram() {
  let src = '';
  for (let i = 0; i < 30; i++) {
    src += `var v${i}: int = ${i}\n`;
  }
  for (let i = 0; i < 10; i++) {
    src += `\non Bumped {\n`;
    for (let j = 0; j < 30; j++) {
      src += `  v${j} = v${j} + ${i + 1}\n`;
    }
    src += `}\n`;
  }
  for (let i = 0; i < 30; i++) {
    src += `out r${i} = v${i}\n`;
  }
  return src;
}

const CHIP_PROGRAM = `chip Add(a: int, b: int) -> (result: int) {
  out result = a + b
}
chip Mul(a: int, b: int) -> (result: int) {
  out result = a * b
}
chip Square(x: int) -> (result: int) {
  out result = x * x
}

var total: int = 0
on RoundStart {
  let a = Add(1, 2)
  let b = Mul(a.result, 3)
  let c = Square(b.result)
  let d = Add(c.result, a.result)
  let e = Mul(d.result, 2)
  total = e.result
}
out sum = total`;

const IMPORT_PROGRAM_MAIN = `import { Add, Mul } from "mathlib"
import * as util from "helpers"

var total: int = 0
on RoundStart {
  let a = Add(1, 2)
  let b = Mul(a.result, 3)
  let c = util.Double(b.result)
  total = c.result
}
out result = total`;

const IMPORT_FILES = JSON.stringify({
  "mathlib.ws": `chip Add(a: int, b: int) -> (result: int) {
  out result = a + b
}
chip Mul(a: int, b: int) -> (result: int) {
  out result = a * b
}
`,
  "helpers.ws": `chip Double(x: int) -> (result: int) {
  out result = x + x
}
chip Triple(x: int) -> (result: int) {
  out result = x + x + x
}
`
});

const MOD_PROGRAM = `var a: int = 0
var b: int = 0
var c: int = 0
mod inc(v: *int) { v = v + 1 }
mod add(v: *int, n: int) { v = v + n }
on Bumped {
  inc(a)
  inc(b)
  inc(c)
  add(a, 10)
  add(b, 20)
  add(c, 30)
  inc(a)
  inc(b)
  inc(c)
}
out ra = a
out rb = b
out rc = c`;

const INTERP_PROGRAM = `var name: string = "world"
var score: int = 42
var health: float = 99.5
on RoundStart {
  name = "player"
  score = score + 1
}
out msg = "Hello \${name}! Score: \${score}, HP: \${health}"
out detail = "Stats: \${score + 1} / \${health * 2.0}"`;

// ── Benchmark harness ───────────────────────────────────────────────

function bench(label, fn, iterations = 200) {
  // Warmup
  for (let i = 0; i < 10; i++) fn();

  const times = [];
  for (let i = 0; i < iterations; i++) {
    const start = performance.now();
    fn();
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  const p95 = times[Math.floor(times.length * 0.95)];
  const mean = times.reduce((a, b) => a + b) / times.length;
  return { label, median, p95, mean, min: times[0], max: times[times.length - 1] };
}

// ── Run benchmarks ──────────────────────────────────────────────────

const benchmarks = [
  // Diagnostics (lex + parse + resolve + typecheck)
  {
    name: 'diagnostics/small',
    fn: (wasm) => () => wasm.wirescript_diagnostics(SMALL_PROGRAM),
  },
  {
    name: 'diagnostics/medium',
    fn: (wasm) => () => wasm.wirescript_diagnostics(MEDIUM_PROGRAM),
  },
  {
    name: 'diagnostics/large',
    fn: (wasm) => () => wasm.wirescript_diagnostics(LARGE_PROGRAM),
  },
  {
    name: 'diagnostics/chips',
    fn: (wasm) => () => wasm.wirescript_diagnostics(CHIP_PROGRAM),
  },
  {
    name: 'diagnostics/imports',
    fn: (wasm) => () => wasm.wirescript_diagnostics(IMPORT_PROGRAM_MAIN, IMPORT_FILES),
  },
  {
    name: 'diagnostics/mods',
    fn: (wasm) => () => wasm.wirescript_diagnostics(MOD_PROGRAM),
  },
  {
    name: 'diagnostics/interp',
    fn: (wasm) => () => wasm.wirescript_diagnostics(INTERP_PROGRAM),
  },

  // Completions (lex + parse + resolve + typecheck + completions)
  {
    name: 'completions/medium',
    fn: (wasm) => () => wasm.wirescript_completions(MEDIUM_PROGRAM, 15, 5),
  },
  {
    name: 'completions/chips',
    fn: (wasm) => () => wasm.wirescript_completions(CHIP_PROGRAM, 12, 5),
  },

  // Hover
  {
    name: 'hover/medium',
    fn: (wasm) => () => wasm.wirescript_hover(MEDIUM_PROGRAM, 1, 5),
  },
  {
    name: 'hover/chips',
    fn: (wasm) => () => wasm.wirescript_hover(CHIP_PROGRAM, 12, 10),
  },

  // Full compile (lex + parse + resolve + typecheck + lower + emit)
  {
    name: 'compile/small',
    fn: (wasm) => () => { try { wasm.wirescript_compile(SMALL_PROGRAM); } catch {} },
  },
  {
    name: 'compile/medium',
    fn: (wasm) => () => { try { wasm.wirescript_compile(MEDIUM_PROGRAM); } catch {} },
  },
  {
    name: 'compile/large',
    fn: (wasm) => () => { try { wasm.wirescript_compile(LARGE_PROGRAM); } catch {} },
  },
  {
    name: 'compile/chips',
    fn: (wasm) => () => { try { wasm.wirescript_compile(CHIP_PROGRAM); } catch {} },
    iterations: 20,
  },
  {
    name: 'compile/imports',
    fn: (wasm) => () => { try { wasm.wirescript_compile(IMPORT_PROGRAM_MAIN, null, IMPORT_FILES); } catch {} },
    iterations: 20,
  },
  {
    name: 'compile/mods',
    fn: (wasm) => () => { try { wasm.wirescript_compile(MOD_PROGRAM); } catch {} },
  },

  // Format
  {
    name: 'format/medium',
    fn: (wasm) => () => wasm.wirescript_format(MEDIUM_PROGRAM, 2, false),
  },
  {
    name: 'format/large',
    fn: (wasm) => () => wasm.wirescript_format(LARGE_PROGRAM, 2, false),
  },

  // Definition / References
  {
    name: 'definition/medium',
    fn: (wasm) => () => wasm.wirescript_definition(MEDIUM_PROGRAM, 15, 5),
  },
  {
    name: 'references/medium',
    fn: (wasm) => () => wasm.wirescript_references(MEDIUM_PROGRAM, 1, 5),
  },
];

console.log(`\nRunning ${benchmarks.length} benchmarks...\n`);

const results = [];
for (const b of benchmarks) {
  const iters = b.iterations || 500;
  process.stdout.write(`  ${b.name} (${iters}x)...`);
  const base = bench('baseline', b.fn(baseline), iters);
  const opt = bench('optimized', b.fn(optimized), iters);
  const speedup = ((base.median - opt.median) / base.median * 100);
  results.push({
    name: b.name,
    baseMedian: base.median,
    optMedian: opt.median,
    baseP95: base.p95,
    optP95: opt.p95,
    speedup,
  });
  const arrow = speedup > 0 ? '↓' : '↑';
  console.log(` ${base.median.toFixed(3)}ms → ${opt.median.toFixed(3)}ms (${speedup > 0 ? '+' : ''}${speedup.toFixed(1)}% ${arrow})`);
}

// ── Print results table ─────────────────────────────────────────────

console.log('\n' + '─'.repeat(95));
console.log(
  'Benchmark'.padEnd(28) +
  'Baseline (ms)'.padStart(14) +
  'Optimized (ms)'.padStart(15) +
  'Δ (ms)'.padStart(12) +
  'Speedup'.padStart(10) +
  'p95 base'.padStart(10) +
  'p95 opt'.padStart(10)
);
console.log('─'.repeat(95));

for (const r of results) {
  const delta = r.baseMedian - r.optMedian;
  const pct = r.speedup;
  const sign = pct > 0 ? '+' : '';
  console.log(
    r.name.padEnd(28) +
    r.baseMedian.toFixed(3).padStart(14) +
    r.optMedian.toFixed(3).padStart(15) +
    delta.toFixed(3).padStart(12) +
    `${sign}${pct.toFixed(1)}%`.padStart(10) +
    r.baseP95.toFixed(3).padStart(10) +
    r.optP95.toFixed(3).padStart(10)
  );
}
console.log('─'.repeat(95));

// Summary
const avgSpeedup = results.reduce((a, r) => a + r.speedup, 0) / results.length;
const compileResults = results.filter(r => r.name.startsWith('compile/'));
const compileAvg = compileResults.reduce((a, r) => a + r.speedup, 0) / compileResults.length;
const diagResults = results.filter(r => r.name.startsWith('diagnostics/'));
const diagAvg = diagResults.reduce((a, r) => a + r.speedup, 0) / diagResults.length;

console.log(`\nAvg speedup (all):          ${avgSpeedup > 0 ? '+' : ''}${avgSpeedup.toFixed(1)}%`);
console.log(`Avg speedup (compile):      ${compileAvg > 0 ? '+' : ''}${compileAvg.toFixed(1)}%`);
console.log(`Avg speedup (diagnostics):  ${diagAvg > 0 ? '+' : ''}${diagAvg.toFixed(1)}%`);

// Binary size comparison
const baseSize = readFileSync('./pkg-baseline/wasm_bg.wasm').byteLength;
const optSize = readFileSync('./pkg/wasm_bg.wasm').byteLength;
const sizeDelta = optSize - baseSize;
const sizePct = (sizeDelta / baseSize * 100).toFixed(1);
console.log(`\nWASM binary size: ${(baseSize/1024).toFixed(0)}KB → ${(optSize/1024).toFixed(0)}KB (${sizeDelta > 0 ? '+' : ''}${sizePct}%, ${sizeDelta > 0 ? '+' : ''}${(sizeDelta/1024).toFixed(1)}KB)`);
