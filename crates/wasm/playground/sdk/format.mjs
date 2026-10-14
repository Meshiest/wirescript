#!/usr/bin/env node
// wirescript formatter — run with: node format.mjs <file.ws> [--stdout]

import { readFileSync, writeFileSync } from 'fs';
import { resolve } from 'path';
import { loadWasm } from './lib.mjs';

async function main() {
  const args = process.argv.slice(2);
  const toStdout = args.includes('--stdout');
  const file = args.find(a => !a.startsWith('-'));

  if (!file) {
    console.error('Usage: node format.mjs <file.ws> [--stdout]');
    process.exit(1);
  }

  const source = readFileSync(resolve(file), 'utf-8');

  const wirescript_format = (await loadWasm())('wirescript_format');

  const formatted = wirescript_format(source, 2, false);

  if (toStdout) {
    process.stdout.write(formatted);
  } else {
    writeFileSync(resolve(file), formatted);
    console.log(`✓ formatted ${file}`);
  }
}

main().catch(e => { console.error(e); process.exit(1); });
