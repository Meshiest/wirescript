// What the SDK's command-line tools share: the compiler, and the files a
// program imports and the prefabs it embeds, read from disk.

import { existsSync, readFileSync, readdirSync } from 'fs';
import { dirname, join, relative, resolve, sep } from 'path';
import { fileURLToPath } from 'url';

const here = dirname(fileURLToPath(import.meta.url));

/** The compiler, loaded: a function giving its export by name. */
export async function loadWasm() {
  const mod = await import('./pkg/wasm.js');
  // nodejs-target pkg exports directly; web-target pkg needs init().
  if (typeof mod.default === 'function') {
    await mod.default({ module_or_path: readFileSync(join(here, 'pkg', 'wasm_bg.wasm')) });
  }
  return (name) => mod[name] ?? mod.default?.[name];
}

/**
 * Every other `.ws` file in `file`'s folder and the folders below it, as the
 * JSON map the compiler reads its imports from: the path from `file`'s folder,
 * with `/` separators, to the file's source.
 */
export function fileMapJson(file) {
  const self = resolve(file);
  const base = dirname(self);
  const map = {};
  const walk = (dir) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (!entry.name.startsWith('.') && entry.name !== 'node_modules') walk(full);
      } else if (entry.name.endsWith('.ws') && full !== self) {
        map[relative(base, full).split(sep).join('/')] = readFileSync(full, 'utf-8');
      }
    }
  };
  walk(base);
  return JSON.stringify(map);
}

/**
 * The `.brz` prefabs `source`, the text of `file`, embeds by a `$./` reference
 * that names a file on disk, as the JSON registry the compiler reads: the
 * reference to the file's bytes.
 */
export function prefabsJson(file, source) {
  const base = dirname(resolve(file));
  const registry = {};
  for (const [, ref] of source.matchAll(/\$(\.\/[A-Za-z0-9_./-]+\.brz)/g)) {
    const full = join(base, ref);
    if (existsSync(full)) registry[ref] = Array.from(readFileSync(full));
  }
  return JSON.stringify(registry);
}
