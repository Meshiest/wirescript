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

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { filesUnder, mdLines, repoRoot, slug, writeOrCheck } from "./lib.mjs";

const docsDir = join(repoRoot, "docs", "src");

const OPEN = "<!-- toc -->";
const CLOSE = "<!-- /toc -->";

// The README's Table of Contents lists the other PAGES, not its own sections;
// it is hand-written and stays that way.
// README is the book's landing page; SUMMARY is generated and CHANGELOG is
// copied in by the book build, so neither is an authored page.
const SKIP = new Set(["README.md", "SUMMARY.md", "CHANGELOG.md"]);

/** Heading text as it should read in the list: markdown emphasis stripped, code spans kept. */
function linkText(heading) {
  return heading.replace(/\*\*|__/g, "").trim();
}

/**
 * The `##` headings outside fenced code blocks, each with its anchor: a
 * heading whose slug an earlier `#`-`####` heading already took gets `-1`,
 * `-2`, as GitHub and the playground number them.
 */
function sections(text) {
  const out = [];
  const seen = new Map();
  for (const { line, fence, fenced } of mdLines(text)) {
    if (fence || fenced) continue;
    const m = line.match(/^(#{1,4}) +(.+?)\s*$/);
    if (!m) continue;
    const base = slug(m[2]);
    const n = seen.get(base) || 0;
    seen.set(base, n + 1);
    if (m[1] === "##" && m[2] !== "Contents") out.push({ name: m[2], anchor: n ? `${base}-${n}` : base });
  }
  return out;
}

/** `lines` without an existing Contents block. */
function withoutToc(lines) {
  const open = lines.indexOf(OPEN);
  const close = open === -1 ? -1 : lines.indexOf(CLOSE, open);
  return close === -1 ? lines : [...lines.slice(0, open), ...lines.slice(close + 1)];
}

/**
 * Splice the generated block into `text`, replacing an existing marked block or
 * inserting one above the first `##` heading. Returns the new text, or `null`
 * when the page has no sections to list.
 */
function withToc(text) {
  const eol = text.includes("\r\n") ? "\r\n" : "\n";
  const lines = text.split(/\r?\n/);
  // The Contents block's own heading is no section of the page.
  const names = sections(withoutToc(lines).join("\n"));
  if (names.length < 2) return null; // a single-section page needs no index

  const block = [
    OPEN,
    "## Contents",
    "",
    ...names.map((n) => `- [${linkText(n.name)}](#${n.anchor})`),
    CLOSE,
  ].join(eol);

  const open = lines.indexOf(OPEN);
  if (open !== -1) {
    const close = lines.indexOf(CLOSE, open);
    if (close === -1) throw new Error("unterminated <!-- toc --> block");
    return [...lines.slice(0, open), block, ...lines.slice(close + 1)].join(eol);
  }

  const first = [...mdLines(text)].find((l) => !l.fence && !l.fenced && /^## +/.test(l.line))?.index ?? -1;
  if (first === -1) return null;
  return [...lines.slice(0, first), block, "", ...lines.slice(first)].join(eol);
}

const check = process.argv.includes("--check");
// `learn/` pages nest one folder deeper; a README is a section index either way.
const pages = filesUnder(docsDir, ".md").filter((f) => !SKIP.has(f.split("/").pop()));

let stale = 0;
let written = 0;
for (const page of pages) {
  const path = join(docsDir, page);
  const text = readFileSync(path, "utf8");
  const next = withToc(text);
  if (next === null || next === text) continue;
  if (!writeOrCheck(path, next, { check, what: `doc-toc: ${page}'s Contents list` })) {
    stale++;
  } else {
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
