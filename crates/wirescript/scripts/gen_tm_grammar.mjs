#!/usr/bin/env node
// Keep the VS Code grammar's word lists (`editors/vscode/syntaxes/
// wirescript.tmLanguage.json`) in step with the Monarch grammar the playground
// uses (`crates/wasm/playground/monarch.js`). One keyword list feeds both
// editors.
//
//   node crates/wirescript/scripts/gen_tm_grammar.mjs           # write
//   node crates/wirescript/scripts/gen_tm_grammar.mjs --check   # CI gate

import { readFileSync } from 'fs';
import { join } from 'path';
import { pathToFileURL } from 'url';
import { repoRoot, writeOrCheck } from './lib.mjs';

const monarch = join(repoRoot, 'crates', 'wasm', 'playground', 'monarch.js');
const { monarchLanguage: g } = await import(pathToFileURL(monarch).href);

// The word lists, rewritten in place in the grammar's JSON text so the file's
// hand formatting stays. `unsafe` and `is` keep their own contextual rules;
// the declaration keywords take the declaration scope.
const tmPath = join(repoRoot, 'editors', 'vscode', 'syntaxes', 'wirescript.tmLanguage.json');
const CONTEXTUAL = new Set(['unsafe', 'is']);
const DECLARATIONS = new Set([
  'var', 'array', 'map', 'static', 'buffer', 'let', 'const', 'fn', 'chip', 'mod',
  'in', 'out', 'ref', 'import', 'type', 'enum',
]);
const alternation = (xs) => [...new Set(xs)].sort().join('|');
const tmLists = {
  'keyword.control.wirescript': g.keywords.filter((k) => !CONTEXTUAL.has(k) && !DECLARATIONS.has(k)),
  'keyword.declaration.wirescript': g.keywords.filter((k) => DECLARATIONS.has(k)),
  'support.type.wirescript': g.typeKeywords,
  'support.function.wirescript': g.builtins,
  'support.constant.event.wirescript': g.events,
  'constant.language.wirescript': g.constants || [],
};
const EVENTS_RULE = `        {
          "name": "support.constant.event.wirescript",
          "match": ""
        }`;

function tmText(current) {
  let text = current.replace(/\r\n/g, '\n');
  if (!text.includes('"support.constant.event.wirescript"')) {
    // The events rule sits beside the builtins one.
    const at = text.indexOf('"name": "support.function.wirescript"');
    const end = text.indexOf('\n        }', at) + '\n        }'.length;
    text = text.slice(0, end) + ',\n' + EVENTS_RULE + text.slice(end);
  }
  for (const [scope, words] of Object.entries(tmLists)) {
    const rule = new RegExp(`("name": "${scope.replace(/\./g, '\\.')}",\\s*"match": )"[^"\\n]*"`);
    if (!rule.test(text)) throw new Error(`gen_tm_grammar: no "${scope}" rule in ${tmPath}`);
    const match = JSON.stringify(`\\b(${alternation(words)})\\b`);
    text = text.replace(rule, (_, head) => head + match);
  }
  return current.includes('\r\n') ? text.replace(/\n/g, '\r\n') : text;
}

const readOr = (path) => {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return '';
  }
};

const check = process.argv.includes('--check');
const fresh = writeOrCheck(tmPath, tmText(readOr(tmPath)), {
  check,
  what: 'gen_tm_grammar: editors/vscode/syntaxes/wirescript.tmLanguage.json',
  fix: 'just vscode-grammar',
});
if (!fresh) process.exit(1);
if (check) {
  console.log('gen_tm_grammar: the VS Code grammar is up to date');
} else {
  console.log(
    `gen_tm_grammar: wrote the VS Code grammar ` +
      `(${g.keywords.length} keywords, ${g.typeKeywords.length} types, ` +
      `${g.builtins.length + g.events.length} builtins)`,
  );
}
