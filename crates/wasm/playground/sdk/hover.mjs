#!/usr/bin/env node
// wirescript hover info — run with: node hover.mjs <file.ws> <line> <col>

import { readFileSync, readdirSync } from 'fs';
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
  const [file, lineStr, colStr] = process.argv.slice(2);
  if (!file || !lineStr) {
    console.error('Usage: node hover.mjs <file.ws> <line> <col>');
    console.error('  line/col are 1-based');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');
  const line = parseInt(lineStr, 10) - 1;
  const col = parseInt(colStr || '1', 10) - 1;

  const { default: init, wirescript_hover } = await import('./pkg/wasm.js');
  const wasmBytes = readFileSync(join(__dirname, 'pkg', 'wasm_bg.wasm'));
  await init({ module_or_path: wasmBytes });

  const result = wirescript_hover(source, line, col, buildFileMap(file));
  if (!result) {
    console.log('(no hover info)');
    process.exit(0);
  }

  try {
    const parsed = JSON.parse(result);
    console.log(parsed.value);
  } catch {
    console.log(result);
  }
}

main().catch(e => { console.error(e); process.exit(1); });
