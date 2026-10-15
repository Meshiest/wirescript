// snippet.js -- turn a documentation code block into a compilable program.
//
// Shared by the book's Compile button (docs/assets/ws-snippet.js) and the
// playground's doc panel. A block's fence words arrive as classes
// (`language-wirescript name=door`); a hidden prelude arrives as the
// `<!-- doc-check-prelude: ... -->` comments right before the block; and a
// library the block imports (`import { escape } from "sanitize"`) is fetched
// as `sanitize.ws` and handed to the compiler with it.

/** The rules a block's fence words ask for (`name=x`, `ignore`, ...). */
export function blockRules(words) {
  const value = (key) => words.find((w) => w.startsWith(`${key}=`))?.slice(key.length + 1) ?? null;
  return {
    ignore: words.includes('ignore'),
    nocompile: words.includes('nocompile'),
    // The block uses gates only an admin can load (debugging aids, chat
    // commands); the rest of its page need not.
    admin: words.includes('admin'),
    name: value('name'),
    expect: value('expect'),
    benchFor: value('bench-for'),
  };
}

/** The fence words of a rendered block, from its `class` list. */
export function wordsFromClasses(classes) {
  return [...classes].filter((c) => c !== 'language-wirescript' && c !== 'hljs' && !c.startsWith('language-'));
}

/** Whether a rendered block is one a reader can compile and download. */
export function isCompilable(rules) {
  return Boolean(rules.name) && !rules.ignore && !rules.nocompile && !rules.expect;
}

/** The module names `source` imports (`import { x } from "sanitize"` gives `sanitize`). */
export function importsOf(source) {
  const names = [];
  // An import's `{ ... }` list may span lines.
  for (const m of source.matchAll(/^\s*import\b[^"']*?["']([^"'\n]+)["']/gm)) names.push(m[1]);
  return names;
}

/**
 * Every library `source` needs, following imports of imports, as the
 * `{ "name.ws": text }` map the compiler's `files_json` takes. `fetchText`
 * resolves a module name to its source.
 */
export async function collectLibraries(source, fetchText) {
  const files = {};
  const queue = importsOf(source);
  while (queue.length) {
    const name = queue.shift();
    const file = `${name.replace(/^\.\//, '')}.ws`;
    if (file in files) continue;
    const text = await fetchText(name);
    files[file] = text;
    queue.push(...importsOf(text));
  }
  return files;
}

/**
 * The hidden prelude before a rendered block: the text of each
 * `<!-- doc-check-prelude: ... -->` comment among the nodes just before
 * `node`, earliest first. Whitespace between them is skipped; anything else
 * ends the search, as it does in doc-check.
 */
export function preludeBefore(node) {
  const parts = [];
  for (let n = node.previousSibling; n; n = n.previousSibling) {
    if (n.nodeType === 3 && !n.textContent.trim()) continue; // whitespace text
    if (n.nodeType !== 8) break; // not a comment
    const m = n.textContent.match(/^\s*doc-check-prelude:\s*\n([\s\S]*)$/);
    if (!m) break;
    parts.unshift(m[1].replace(/\s*$/, '\n'));
  }
  return parts.join('');
}

/** Save `bytes` as a download named `filename`. */
export function downloadBytes(bytes, filename) {
  const url = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
