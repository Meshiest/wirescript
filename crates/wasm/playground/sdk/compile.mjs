#!/usr/bin/env node
// wirescript compiler — run with: node compile.mjs <file.ws> [output.brz]

import { readFileSync, writeFileSync } from 'fs';
import { resolve, basename } from 'path';
import { fileMapJson, loadWasm, prefabsJson } from './lib.mjs';

async function main() {
  const file = process.argv[2];
  if (!file) {
    console.error('Usage: node compile.mjs <file.ws> [output.brz]');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');
  const outFile = process.argv[3] || file.replace(/\.[^.]+$/, '.brz');

  const wirescript_compile = (await loadWasm())('wirescript_compile');

  try {
    const name = basename(file).replace(/\.[^.]+$/, '');
    const bytes = wirescript_compile(source, name, fileMapJson(file), prefabsJson(file, source));
    writeFileSync(resolve(outFile), Buffer.from(bytes));
    console.log(`✓ wrote ${outFile} (${bytes.length} bytes)`);
  } catch (e) {
    console.error(e.toString ? e.toString() : e);
    process.exit(1);
  }
}

main().catch(e => { console.error(e); process.exit(1); });
