#!/usr/bin/env node
// Compiles every ```wirescript block in the Learn series (docs/src/learn/),
// which a reader can download as a save with one click, so each one must
// really build. Run via `just doc-check`, after `check_docs.mjs` has
// parse-checked every page's blocks.
//
// Rules, from the fence's info string (words after `wirescript`):
//   name=x          required on every compiled block, unique across the series;
//                   the module (and download) name
//   ignore          skipped, as in check_docs.mjs
//   nocompile       a fragment shown for reading, never offered as a download
//   expect=WS0nn    must report that code, as an error or a warning (a page
//                   that shows a diagnostic on purpose)
//   bench-for=x     a test bench for block `x`: its outputs must be x's
//                   inputs, and its inputs x's outputs, by name and type
//   admin           the block may use gates only an admin can load
//   file=path       the block is a copy of that file (relative to the page),
//                   and must match it exactly
// A compiled block fails on any error or warning, and, unless it or its page
// (in learn.js) is marked admin, on any gate listed in docs/admin-gates.json.
// The same holds for every inline prefab (`$```…```) inside it, compiled on
// its own, since its gates never reach the outer program's IR. Every asset
// reference (`$Type/Name`) must name an asset the game's inventory lists.
// Library files in docs/src/learn/lib/ sit beside every block, so
// `import { escape } from "sanitize"` resolves as it does in the browser.

import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { binPath, filesUnder, isMain, repoRoot, wsBlocks } from './lib.mjs';
// The browser's Compile button reads the same words with the same rules.
import { blockRules } from '../../wasm/playground/snippet.js';

export { blockRules };

/**
 * The side pins a compiled program exposes, read from its `.wsir`: `inputs`
 * and `outputs`, each a sorted list of `name:type@side` (`a:bool@left`).
 */
export function sidePins(ir) {
  const inputs = [];
  const outputs = [];
  for (const line of ir.split(/\r?\n/)) {
    const input = line.match(/^\s*%(\w+) = in (\S+) \{[^}]*side = (\w+)/);
    if (input) inputs.push(`${input[1]}:${input[2]}@${input[3]}`);
    const output = line.match(/^\s*out %(\w+) (\S+)(?: = [^{]*)? \{[^}]*side = (\w+)/);
    if (output) outputs.push(`${output[1]}:${output[2]}@${output[3]}`);
  }
  return { inputs: inputs.sort(), outputs: outputs.sort() };
}

/**
 * Why `bench` cannot drive `subject` pin for pin, or null when it can: every
 * bench output feeds a subject input of the same name and type, and every
 * subject output feeds a bench input. Only left and right pins pair up; a
 * bench's own controls (a start button, a result display) sit on its top and
 * bottom.
 */
export function benchMismatch(bench, subject) {
  const strip = (pins) =>
    pins
      .filter((p) => /@(left|right)$/.test(p))
      .map((p) => p.replace(/@\w+$/, ''))
      .sort()
      .join(', ');
  const problems = [];
  if (strip(bench.outputs) !== strip(subject.inputs)) {
    problems.push(`bench outputs [${strip(bench.outputs)}] != inputs [${strip(subject.inputs)}]`);
  }
  if (strip(bench.inputs) !== strip(subject.outputs)) {
    problems.push(`bench inputs [${strip(bench.inputs)}] != outputs [${strip(subject.outputs)}]`);
  }
  return problems.length ? problems.join('; ') : null;
}

/** The gate classes in `ir` that `adminGates` lists. */
export function adminGatesIn(ir, adminGates) {
  return adminGates.filter((cls) => new RegExp(`= ${cls}\\b`).test(ir));
}

/**
 * The source of every inline prefab (`` $```…``` ``) in `code`, nested ones
 * included, outermost first. The compiler embeds each as a finished save, so
 * its gates never reach the outer program's IR; the check compiles them on
 * their own. Strings and `//` comments are skipped and nested fences counted,
 * as the lexer does.
 */
export function inlinePrefabs(code) {
  const found = [];
  const fence = (i) => code.startsWith('```', i);
  // Skip a string or a line comment starting at `i`; returns the index after
  // it, or `i` when there is none.
  const skip = (i) => {
    const c = code[i];
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < code.length && code[j] !== c) j += code[j] === '\\' ? 2 : 1;
      return j + 1;
    }
    if (c === '/' && code[i + 1] === '/') {
      const nl = code.indexOf('\n', i);
      return nl === -1 ? code.length : nl;
    }
    return i;
  };
  for (let i = 0; i < code.length; ) {
    const after = skip(i);
    if (after !== i) {
      i = after;
      continue;
    }
    if (code[i] === '$' && fence(i + 1)) {
      const start = i + 4;
      let depth = 1;
      let j = start;
      while (j < code.length) {
        const past = skip(j);
        if (past !== j) {
          j = past;
          continue;
        }
        if (code[j] === '$' && fence(j + 1)) {
          depth++;
          j += 4;
        } else if (fence(j)) {
          if (--depth === 0) break;
          j += 3;
        } else j++;
      }
      const body = code.slice(start, j);
      found.push(body, ...inlinePrefabs(body));
      i = j + 3;
      continue;
    }
    i++;
  }
  return found;
}

/**
 * The asset references (`$Type/Name`) in `code` that the game's asset
 * inventory does not list. The compiler accepts any name, so a misspelled
 * sound or item would compile and then do nothing in game. Prefab paths
 * (`$./x.brz`) are not assets.
 */
export function unknownAssets(code, inventory) {
  const known = new Map(inventory.assetTypes.map((t) => [t.type, new Set(t.assets.map((a) => a.name))]));
  const unknown = [];
  for (const [ref, type, name] of code.matchAll(/\$([A-Za-z_]\w*)\/(\w+)/g)) {
    if (!known.get(type)?.has(name)) unknown.push(ref);
  }
  return unknown;
}

/** `text` without terminal colour codes. */
const plain = (text) => (text ?? '').replace(/\x1b\[[0-9;]*m/g, '');

async function main() {
  const docsDir = join(repoRoot, 'docs', 'src');
  const learnDir = join(docsDir, 'learn');
  const cli = binPath('wirescript');
  if (!existsSync(cli)) {
    console.error(`learn-check: ${cli} does not exist -- run \`just check-bin\` first`);
    process.exit(1);
  }
  const { gates: adminGates } = JSON.parse(readFileSync(join(repoRoot, 'docs', 'admin-gates.json'), 'utf8'));
  const assets = JSON.parse(readFileSync(join(repoRoot, 'crates', 'wirescript', 'data', 'asset_inventory.simple.json'), 'utf8'));
  const learnJs = join(repoRoot, 'crates', 'wasm', 'playground', 'learn.js');
  const { LEARN } = await import(pathToFileURL(learnJs).href);
  const adminPages = new Set(LEARN.filter((p) => p.admin).map((p) => p.file.slice('docs/'.length)));

  const scratch = mkdtempSync(join(tmpdir(), 'ws-learncheck-'));
  const libDir = join(learnDir, 'lib');
  // The browser fetches an import `x` as `learn/lib/x.ws`, so only top-level
  // library files are importable.
  const libs = existsSync(libDir) ? filesUnder(libDir, '.ws').filter((l) => !l.includes('/')) : [];
  const libNames = new Set(libs.map((l) => l.replace(/\.ws$/, '')));
  // Each block compiles alone beside the libraries, as in the browser: one
  // lesson's block can never satisfy another's import.
  const dirFor = (name) => {
    const dir = join(scratch, name);
    mkdirSync(dir);
    for (const lib of libs) copyFileSync(join(libDir, lib), join(dir, lib));
    return dir;
  };

  const failures = [];
  const compiled = new Map(); // name -> { where, pins }
  const benches = [];
  let built = 0;
  let expected = 0;
  let prefabs = 0;

  const pages = existsSync(learnDir) ? filesUnder(docsDir, '.md').filter((f) => f.startsWith('learn/')) : [];
  for (const page of pages) {
    for (const b of wsBlocks(readFileSync(join(docsDir, page), 'utf8'))) {
      const rules = blockRules(b.words);
      const where = `${page}:${b.startLine}`;
      if (rules.file) {
        const copied = join(docsDir, dirname(page), rules.file);
        if (!existsSync(copied)) failures.push(`${where}: file=${rules.file} does not exist`);
        else if (readFileSync(copied, 'utf8').replace(/\r\n/g, '\n').trimEnd() !== b.code.trimEnd()) {
          failures.push(`${where}: the block no longer matches ${rules.file}; copy the file into it`);
        }
      }
      if (rules.ignore || rules.nocompile) continue;
      if (!rules.name) {
        failures.push(`${where}: a compiled block needs a name= tag`);
        continue;
      }
      if (!/^[\w-]+$/.test(rules.name)) {
        failures.push(`${where}: name=${rules.name} must be letters, digits, - and _ (it names the download)`);
        continue;
      }
      if (compiled.has(rules.name) || libNames.has(rules.name)) {
        failures.push(`${where}: name=${rules.name} is already used`);
        continue;
      }
      const dir = dirFor(rules.name);
      const file = join(dir, `${rules.name}.ws`);
      writeFileSync(file, (b.prelude || '') + b.code + '\n');
      const run = spawnSync(cli, ['compile', file, '-o', join(dir, `${rules.name}.brz`), '--dump-ir'], {
        encoding: 'utf8',
      });
      // The IR is stdout; errors and warnings are stderr.
      const ir = plain(run.stdout);
      const diagnostics = plain(run.stderr);
      const ok = run.status === 0;

      // An error stops the compile; a warning lets it finish. Either way the
      // block must report the code it shows.
      if (rules.expect) {
        if (!diagnostics.includes(`[${rules.expect}]`)) {
          failures.push(
            `${where}: expected ${rules.expect}, ${ok && !diagnostics.trim() ? 'but it compiled cleanly' : `got:\n${diagnostics.trim()}`}`,
          );
        } else expected++;
        compiled.set(rules.name, { where });
        continue;
      }
      if (!ok) {
        failures.push(`${where} (${rules.name}): does not compile:\n${diagnostics.trim()}`);
        continue;
      }
      // The pin and admin checks read the IR; without one they would pass on
      // nothing.
      if (!/^module @/m.test(ir)) {
        failures.push(`${where} (${rules.name}): the compiler printed no IR to check`);
        continue;
      }
      const warnings = diagnostics.split(/\r?\n/).filter((l) => /\bWARN\b/.test(l));
      if (warnings.length) failures.push(`${where} (${rules.name}): warns:\n${warnings.join('\n')}`);
      if (!adminPages.has(page) && !rules.admin) {
        const found = adminGatesIn(ir, adminGates);
        if (found.length) {
          failures.push(`${where} (${rules.name}): admin-only gate(s) ${found.join(', ')} on a page not marked admin`);
        }
      }
      const badAssets = unknownAssets(b.code, assets);
      if (badAssets.length) failures.push(`${where} (${rules.name}): unknown asset(s) ${badAssets.join(', ')}`);
      // Each inline prefab is a program of its own: it must compile cleanly
      // and, on a non-admin page, use no admin gate either.
      inlinePrefabs(b.code).forEach((body, i) => {
        const inner = join(dir, `${rules.name}-prefab${i + 1}.ws`);
        writeFileSync(inner, body + '\n');
        const sub = spawnSync(cli, ['compile', inner, '-o', join(dir, `${rules.name}-prefab${i + 1}.brz`), '--dump-ir'], {
          encoding: 'utf8',
        });
        const label = `${where} (${rules.name}, inline prefab ${i + 1})`;
        const subDiagnostics = plain(sub.stderr);
        if (sub.status !== 0) {
          failures.push(`${label}: does not compile:\n${subDiagnostics.trim()}`);
          return;
        }
        const subWarnings = subDiagnostics.split(/\r?\n/).filter((l) => /\bWARN\b/.test(l));
        if (subWarnings.length) failures.push(`${label}: warns:\n${subWarnings.join('\n')}`);
        if (!adminPages.has(page) && !rules.admin) {
          const found = adminGatesIn(plain(sub.stdout), adminGates);
          if (found.length) failures.push(`${label}: admin-only gate(s) ${found.join(', ')} on a page not marked admin`);
        }
        prefabs++;
      });
      compiled.set(rules.name, { where, pins: sidePins(ir) });
      if (rules.benchFor) benches.push({ name: rules.name, subject: rules.benchFor, where });
      built++;
    }
  }

  for (const bench of benches) {
    const subject = compiled.get(bench.subject);
    if (!subject?.pins) {
      failures.push(`${bench.where} (${bench.name}): bench-for=${bench.subject} names no compiled block`);
      continue;
    }
    const sided = (pins) => [...pins.inputs, ...pins.outputs].filter((p) => /@(left|right)$/.test(p));
    if (sided(subject.pins).length === 0) {
      failures.push(`${bench.where} (${bench.name}): ${bench.subject} has no side pins for a bench to drive`);
      continue;
    }
    const why = benchMismatch(compiled.get(bench.name).pins, subject.pins);
    if (why) failures.push(`${bench.where} (${bench.name}): cannot drive ${bench.subject}: ${why}`);
  }

  // An empty or mis-pathed series would otherwise pass vacuously.
  if (built === 0) failures.push('no compiled blocks found under docs/src/learn/');

  console.log(
    `learn-check: ${built} compiled (+${prefabs} inline prefabs), ${expected} expected diagnostics, ${benches.length} benches, ${failures.length} failed`,
  );
  for (const f of failures) console.log(`\n--- ${f}`);
  process.exit(failures.length === 0 ? 0 : 1);
}

if (isMain(import.meta.url)) await main();
