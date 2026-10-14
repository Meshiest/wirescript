// Helpers the repository's scripts share.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** The repository root. */
export const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

/** The value after `name` among `args` (the command line by default), or `dflt`. */
export function opt(name, dflt, args = process.argv.slice(2)) {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : dflt;
}

/** Whether the module whose `import.meta.url` is `url` is the script node ran. */
export function isMain(url) {
  return Boolean(process.argv[1]) && url === pathToFileURL(process.argv[1]).href;
}

/** A binary this workspace builds, under `target/<profile>`. */
export function binPath(name, profile = 'release') {
  return join(repoRoot, 'target', profile, process.platform === 'win32' ? `${name}.exe` : name);
}

/**
 * Write `text` to `path`, or with `check` report whether `path` already holds
 * it. A stale file is reported as `what`, naming the recipe `fix` that
 * regenerates it. Returns whether the file is (now) up to date.
 */
export function writeOrCheck(path, text, { check = process.argv.includes('--check'), what = path, fix } = {}) {
  if (!check) {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
    return true;
  }
  let current = '';
  try {
    current = readFileSync(path, 'utf8');
  } catch {
    // A missing file counts as stale.
  }
  if (current === text) return true;
  console.error(`${what} is stale${fix ? `, run \`${fix}\`` : ''}`);
  return false;
}

/**
 * Each line of the markdown `text` (either newline style), with where it sits
 * relative to fenced code blocks: `fence` when it opens or closes a block,
 * `fenced` when it is a block's body, and the open block's `indent` and
 * `info` string. A block opens with ``` or ~~~ at any indent and closes with
 * the same marker at the same indent.
 */
export function* mdLines(text) {
  const lines = text.split(/\r?\n/);
  let open = null;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    const m = line.match(/^(\s*)(```|~~~)(.*)$/);
    if (open) {
      const closes = m && m[1] === open.indent && m[2] === open.marker && m[3].trim() === '';
      yield { line, index, fence: Boolean(closes), fenced: !closes, indent: open.indent, info: open.info };
      if (closes) open = null;
    } else if (m) {
      open = { indent: m[1], marker: m[2], info: m[3].trim() };
      yield { line, index, fence: true, fenced: false, indent: open.indent, info: open.info };
    } else {
      yield { line, index, fence: false, fenced: false, indent: '', info: '' };
    }
  }
}
