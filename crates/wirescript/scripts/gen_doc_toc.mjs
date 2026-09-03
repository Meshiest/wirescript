#!/usr/bin/env node
// Generates the "Contents" list at the top of every language-reference page, so
// a long doc can be navigated without scrolling it. Each page gets one entry per
// `##` section, between `<!-- toc -->` / `<!-- /toc -->` markers placed just
// above the page's first `##` heading (after its intro prose). Run via
// `just doc-toc`; `just doc-check` runs it with `--check`, so a page whose
// sections changed without regenerating fails the build instead of shipping a
// stale list.
//
// Subsections (`###`) are deliberately left out: the longest page has 58 of
// them, which buries the sections you are actually choosing between.
//
// The anchors use GitHub's slug rule, which the playground's renderer mirrors
// (`playground/docs.js` assigns heading ids with the same function) so a
// Contents link works both on GitHub and in the doc browser.

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const docsDir = join(repoRoot, "docs", "src");

const OPEN = "<!-- toc -->";
const CLOSE = "<!-- /toc -->";

// The README's Table of Contents lists the other PAGES, not its own sections;
// it is hand-written and stays that way.
const SKIP = new Set(["README.md"]);

/** GitHub's heading slug: strip markdown, drop punctuation, spaces to hyphens. */
export function slug(heading) {
  return heading
    .replace(/`/g, "")
    .replace(/\*\*|__|\*|_/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .trim()
    .toLowerCase()
    .replace(/[^\w\- ]+/g, "")
    .replace(/ /g, "-");
}

/** Heading text as it should read in the list: markdown emphasis stripped, code spans kept. */
function linkText(heading) {
  return heading.replace(/\*\*|__/g, "").trim();
}

/** `##` headings, skipping any inside a fenced code block. */
function sections(lines) {
  const out = [];
  let fenced = false;
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    if (fenced) continue;
    const m = line.match(/^## +(.+?)\s*$/);
    if (m && m[1] !== "Contents") out.push(m[1]);
  }
  return out;
}

/**
 * Splice the generated block into `text`, replacing an existing marked block or
 * inserting one above the first `##` heading. Returns the new text, or `null`
 * when the page has no sections to list.
 */
function withToc(text) {
  const eol = text.includes("\r\n") ? "\r\n" : "\n";
  const lines = text.split(/\r?\n/);
  const names = sections(lines);
  if (names.length < 2) return null; // a single-section page needs no index

  const block = [
    OPEN,
    "## Contents",
    "",
    ...names.map((n) => `- [${linkText(n)}](#${slug(n)})`),
    CLOSE,
  ].join(eol);

  const open = lines.indexOf(OPEN);
  if (open !== -1) {
    const close = lines.indexOf(CLOSE, open);
    if (close === -1) throw new Error("unterminated <!-- toc --> block");
    return [...lines.slice(0, open), block, ...lines.slice(close + 1)].join(eol);
  }

  let fenced = false;
  let first = -1;
  for (let i = 0; i < lines.length; i++) {
    if (/^\s*(```|~~~)/.test(lines[i])) fenced = !fenced;
    if (!fenced && /^## +/.test(lines[i])) {
      first = i;
      break;
    }
  }
  if (first === -1) return null;
  return [...lines.slice(0, first), block, "", ...lines.slice(first)].join(eol);
}

const check = process.argv.includes("--check");
const pages = readdirSync(docsDir)
  .filter((f) => f.endsWith(".md") && !SKIP.has(f))
  .sort();

let stale = 0;
let written = 0;
for (const page of pages) {
  const path = join(docsDir, page);
  const text = readFileSync(path, "utf8");
  const next = withToc(text);
  if (next === null || next === text) continue;
  if (check) {
    stale++;
    console.error(`doc-toc: ${page} has a stale or missing Contents list`);
  } else {
    writeFileSync(path, next);
    written++;
    console.log(`doc-toc: updated ${page}`);
  }
}

if (check) {
  if (stale) {
    console.error(`doc-toc: ${stale} page(s) out of date -- run \`just doc-toc\``);
    process.exit(1);
  }
  console.log(`doc-toc: ${pages.length} page(s) up to date`);
} else {
  console.log(`doc-toc: ${written} page(s) updated, ${pages.length} checked`);
}
