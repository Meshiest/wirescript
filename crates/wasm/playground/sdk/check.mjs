#!/usr/bin/env node
// wirescript type checker — run with: node check.mjs <file.ws>

import { readFileSync, readdirSync } from 'fs';
import { resolve, dirname, join, basename } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

async function loadWasm() {
  const mod = await import('./pkg/wasm.js');
  // nodejs-target pkg exports directly; web-target pkg needs init().
  if (typeof mod.default === 'function') {
    const wasmBytes = readFileSync(join(__dirname, 'pkg', 'wasm_bg.wasm'));
    await mod.default({ module_or_path: wasmBytes });
  }
  const wirescript_diagnostics = mod.wirescript_diagnostics ?? mod.default?.wirescript_diagnostics;
  return { wirescript_diagnostics };
}

function buildFileMap(file) {
  const dir = dirname(resolve(file));
  const name = basename(resolve(file));
  const map = {};
  try {
    for (const entry of readdirSync(dir)) {
      if (entry.endsWith('.ws') && entry !== name) {
        map[entry] = readFileSync(join(dir, entry), 'utf-8');
      }
    }
  } catch {}
  return JSON.stringify(map);
}

async function main() {
  const file = process.argv[2];
  if (!file) {
    console.error('Usage: node check.mjs <file.ws>');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');
  const { wirescript_diagnostics } = await loadWasm();

  const json = wirescript_diagnostics(source, buildFileMap(file));
  const diags = JSON.parse(json);

  if (diags.length === 0) {
    console.log(`✓ ${file}: no errors`);
    process.exit(0);
  }

  let hasError = false;
  for (const d of diags) {
    const level = d.severity === 'error' ? '\x1b[31mERROR\x1b[0m' : '\x1b[33mWARN\x1b[0m';
    console.log(`${level} [${d.code}] ${d.message} (${file}:${d.startLine + 1}:${d.startCol + 1})`);
    if (d.severity === 'error') hasError = true;
  }

  process.exit(hasError ? 1 : 0);
}

main().catch(e => { console.error(e); process.exit(1); });
