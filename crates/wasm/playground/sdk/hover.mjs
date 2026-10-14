#!/usr/bin/env node
// wirescript hover info — run with: node hover.mjs <file.ws> <line> <col>

import { readFileSync } from 'fs';
import { resolve } from 'path';
import { fileMapJson, loadWasm } from './lib.mjs';

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

  const wirescript_hover = (await loadWasm())('wirescript_hover');

  const result = wirescript_hover(source, line, col, fileMapJson(file));
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
