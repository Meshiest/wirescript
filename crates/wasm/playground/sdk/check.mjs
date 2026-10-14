#!/usr/bin/env node
// wirescript type checker — run with: node check.mjs <file.ws>

import { readFileSync } from 'fs';
import { resolve } from 'path';
import { fileMapJson, loadWasm } from './lib.mjs';

async function main() {
  const file = process.argv[2];
  if (!file) {
    console.error('Usage: node check.mjs <file.ws>');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');
  const wirescript_diagnostics = (await loadWasm())('wirescript_diagnostics');

  const json = wirescript_diagnostics(source, fileMapJson(file));
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
