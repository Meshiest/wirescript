// Helpers the repository's scripts share.

import { mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
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

/**
 * Every file under `dir` whose name ends with `ext`, as sorted paths relative
 * to `dir` with `/` separators (`learn/cpu/nand.md`). `skip` names
 * directories to leave out, by their path relative to `dir`.
 */
export function filesUnder(dir, ext, { skip = [] } = {}) {
  const out = [];
  const walk = (rel) => {
    for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
      const path = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        if (!skip.includes(path)) walk(path);
      } else if (entry.name.endsWith(ext)) {
        out.push(path);
      }
    }
  };
  walk('');
  return out.sort();
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

/**
 * Every ```wirescript fenced block in the markdown `text`: its `code`, the
 * 1-based `startLine` of its fence, the words of its `info` string after
 * `wirescript`, whether it is `ignore`d, and the hidden `prelude` of
 * `<!-- doc-check-prelude: ... -->` comments right before it (blank lines
 * between are allowed; several accumulate).
 */
export function wsBlocks(text) {
  const blocks = [];
  let pendingPrelude = "";
  // The lines of an open `<!-- doc-check-prelude:` comment.
  let prelude = null;
  // The wirescript block being read.
  let block = null;
  for (const { line, index, fence, fenced, indent, info } of mdLines(text)) {
    if (block) {
      if (fence) {
        blocks.push({ ...block, code: block.body.join("\n") });
        block = null;
      } else {
        // A block inside a list item is indented, and its body carries that
        // indent.
        block.body.push(indent ? line.slice(indent.length) : line);
      }
      continue;
    }
    if (prelude) {
      if (/-->/.test(line)) {
        pendingPrelude += prelude.join("\n") + "\n";
        prelude = null;
      } else {
        prelude.push(line);
      }
      continue;
    }
    if (fence && /^wirescript\b/.test(info)) {
      // 1-based line of the ``` fence.
      const words = info.replace(/^wirescript\b/, '').split(/[\s,]+/).filter(Boolean);
      block = { startLine: index + 1, words, ignore: words.includes('ignore'), prelude: pendingPrelude, body: [] };
      pendingPrelude = "";
      continue;
    }
    // Hidden prelude comment: <!-- doc-check-prelude: ... -->
    if (!fence && !fenced && /^\s*<!--\s*doc-check-prelude:\s*$/.test(line)) {
      prelude = [];
      continue;
    }
    // A non-prelude, non-blank line clears a dangling prelude (prelude must
    // immediately precede its block, allowing only blank lines between).
    if (line.trim() !== "" && pendingPrelude && !/^\s*<!--/.test(line)) {
      pendingPrelude = "";
    }
  }
  return blocks;
}

/** GitHub's heading slug: strip markdown, drop punctuation, spaces to hyphens. */
export function slug(heading) {
  return heading
    .replace(/`/g, '')
    .replace(/\*\*|__|\*/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .trim()
    .toLowerCase()
    .replace(/[^\w\- ]+/g, '')
    .replace(/ /g, '-');
}
