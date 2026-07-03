#!/usr/bin/env node
// wirescript compiler — run with: node compile.mjs <file.ws> [output.brz]

import { readFileSync, writeFileSync, readdirSync } from 'fs';
import { resolve, basename, dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

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
    console.error('Usage: node compile.mjs <file.ws> [output.brz]');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');
  const outFile = process.argv[3] || file.replace(/\.[^.]+$/, '.brz');

  const { default: init, wirescript_compile } = await import('./pkg/wasm.js');
  const wasmBytes = readFileSync(join(__dirname, 'pkg', 'wasm_bg.wasm'));
  await init({ module_or_path: wasmBytes });

  try {
    const bytes = wirescript_compile(source, basename(file).replace(/\.[^.]+$/, ''), buildFileMap(file));
    writeFileSync(resolve(outFile), Buffer.from(bytes));
    console.log(`✓ wrote ${outFile} (${bytes.length} bytes)`);
  } catch (e) {
    console.error(e.toString ? e.toString() : e);
    process.exit(1);
  }
}

main().catch(e => { console.error(e); process.exit(1); });
