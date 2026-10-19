// Copy the language docs into the extension's `docs/` folder, so they open in
// the editor offline. Run by `npm run build`; the copy is generated (ignored by
// git). Outside a wirescript checkout there is nothing to copy and the existing
// copy, if any, is left alone.
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ext = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(ext, "..", "..");
const src = join(repo, "docs", "src");
if (!existsSync(join(src, "SUMMARY.md"))) {
  console.warn(`copy-docs: no ${join(src, "SUMMARY.md")}; leaving docs/ as it is`);
  process.exit(0);
}

const out = join(ext, "docs");
rmSync(out, { recursive: true, force: true });
cpSync(src, out, { recursive: true });
// The book's Changelog page is the repository's CHANGELOG.
copyFileSync(join(repo, "CHANGELOG.md"), join(out, "CHANGELOG.md"));
// The book's own script and style for the Learn pages' ```wiring diagrams,
// loaded into the markdown preview (package.json). The preview loads scripts
// async, in no fixed order, so the icon table and the renderer that needs it
// go in one file, the table first.
const assets = join(repo, "docs", "assets");
mkdirSync(join(out, "assets"), { recursive: true });
writeFileSync(
  join(out, "assets", "learn-preview.js"),
  [readFileSync(join(assets, "learn-icons.js"), "utf8"), readFileSync(join(assets, "ws-wiring.js"), "utf8")].join("\n"),
);
copyFileSync(join(assets, "learn.css"), join(out, "assets", "learn.css"));
// The listing's license is the repository's.
copyFileSync(join(repo, "LICENSE"), join(ext, "LICENSE"));
console.log(`copy-docs: ${src} -> ${out}`);
