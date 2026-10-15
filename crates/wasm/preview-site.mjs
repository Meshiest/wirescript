#!/usr/bin/env node
// Assemble the playground and the docs book into target/learn-preview and
// serve them, so the Learn lessons can be read with their Compile and Open in
// playground buttons working, the way the published site serves them.
//
//   wasm-pack build crates/wasm --target web --release   # first, for pkg/
//   node crates/wasm/preview-site.mjs [--port 8080] [--no-serve]
//
// Run via `just learn-preview`. Needs mdbook on PATH.

import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, extname, join, normalize, sep } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const wasmDir = dirname(fileURLToPath(import.meta.url));
const repo = join(wasmDir, '..', '..');
const playground = join(wasmDir, 'playground');
const pkg = join(wasmDir, 'pkg');
const out = join(repo, 'target', 'learn-preview');

const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : dflt;
};
const port = Number(arg('--port', '8080'));

if (!existsSync(join(pkg, 'wasm.js'))) {
  console.error('preview-site: crates/wasm/pkg/wasm.js is missing; run `wasm-pack build crates/wasm --target web --release`');
  process.exit(1);
}

rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, 'pkg'), { recursive: true });

// The playground: its page, modules and examples, and the browser compiler.
for (const f of readdirSync(playground)) {
  if (f === 'index.html' || f.endsWith('.js')) copyFileSync(join(playground, f), join(out, f));
}
cpSync(join(playground, 'sdk', 'examples'), join(out, 'sdk', 'examples'), { recursive: true });
for (const f of ['wasm.js', 'wasm_bg.wasm']) copyFileSync(join(pkg, f), join(out, 'pkg', f));
// The SDK download, when build-playground.sh has built it.
if (existsSync(join(wasmDir, 'wirescript-sdk.zip'))) copyFileSync(join(wasmDir, 'wirescript-sdk.zip'), join(out, 'wirescript-sdk.zip'));

// The book at docs/, as the deploy builds it (the CHANGELOG is staged in).
const changelog = join(repo, 'docs', 'src', 'CHANGELOG.md');
copyFileSync(join(repo, 'CHANGELOG.md'), changelog);
const book = spawnSync('mdbook', ['build', join(repo, 'docs'), '-d', join(out, 'docs')], { stdio: 'inherit', shell: process.platform === 'win32' });
rmSync(changelog, { force: true });
if (book.status !== 0) {
  console.error('preview-site: mdbook build failed (is mdbook installed?)');
  process.exit(1);
}

// The raw markdown beside it, which the playground's doc panel loads.
const src = join(repo, 'docs', 'src');
const copyMarkdown = (rel) => {
  for (const entry of readdirSync(join(src, rel), { withFileTypes: true })) {
    const path = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) copyMarkdown(path);
    else if (entry.name.endsWith('.md')) {
      mkdirSync(join(out, 'docs', rel), { recursive: true });
      copyFileSync(join(src, path), join(out, 'docs', path));
    }
  }
};
copyMarkdown('');
copyFileSync(join(repo, 'CHANGELOG.md'), join(out, 'docs', 'CHANGELOG.md'));
spawnSync(process.execPath, [join(playground, 'build-search-index.mjs'), join(out, 'docs'), join(out, 'docs', 'search-index.json')], {
  stdio: 'inherit',
});

console.log(`preview-site: assembled ${out}`);
if (process.argv.includes('--no-serve')) process.exit(0);

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff2': 'font/woff2',
  '.woff': 'font/woff',
  '.ttf': 'font/ttf',
  '.md': 'text/markdown; charset=utf-8',
  '.ws': 'text/plain; charset=utf-8',
};

createServer((req, res) => {
  let path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
  if (path.endsWith('/')) path += 'index.html';
  const file = normalize(join(out, path));
  // Serve only files inside the assembled site.
  if (!file.startsWith(out + sep) || !existsSync(file)) {
    res.writeHead(404).end('not found');
    return;
  }
  res.writeHead(200, { 'Content-Type': TYPES[extname(file)] || 'application/octet-stream' });
  if (req.method === 'HEAD') res.end();
  else res.end(readFileSync(file));
}).listen(port, '127.0.0.1', () => {
  console.log(`preview-site: playground at http://localhost:${port}/ , Learn at http://localhost:${port}/docs/learn/`);
  console.log('preview-site: Ctrl+C to stop');
});
