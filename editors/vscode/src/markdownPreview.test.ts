import { test } from "node:test";
import * as assert from "node:assert/strict";
import * as path from "path";
import { bookLinksToMarkdown, highlight, loadGrammar, repoLinkToDocs } from "./markdownPreview";

const extensionPath = path.resolve(__dirname, "..");

test("colors a block with the extension's grammar", async () => {
  const grammar = await loadGrammar(extensionPath);
  assert.ok(grammar, "the grammar loads");
  const html = highlight(grammar, 'var n: int = 3 // count\non go { n = max(n, 1); Say("a < b ${n}") }\n');
  assert.match(html, /<span class="ws-declaration">var<\/span>/);
  assert.match(html, /<span class="ws-type">int<\/span>/);
  assert.match(html, /<span class="ws-number">3<\/span>/);
  assert.match(html, /<span class="ws-comment">[^<]*count<\/span>/);
  assert.match(html, /<span class="ws-function">max<\/span>/);
  // Text is escaped, and lines stay lines.
  assert.match(html, /a &lt; b/);
  assert.equal(html.split("\n").length, 3);
});

test("book links point at the markdown pages", () => {
  assert.equal(bookLinksToMarkdown('<a href="tools.html">'), '<a href="tools.md">');
  assert.equal(bookLinksToMarkdown('<a href="values/numbers.html#ints">'), '<a href="values/numbers.md#ints">');
  assert.equal(bookLinksToMarkdown('<a href="../index.html">'), '<a href="../README.md">');
  for (const kept of ['<a href="https://x.dev/a.html">', '<a href="/a.html">', '<a href="#a.html">', '<a href="notes.md">']) {
    assert.equal(bookLinksToMarkdown(kept), kept);
  }
});

test("repository changelog links open the bundled changelog", () => {
  const docs = path.resolve("/ext/docs");
  const readme = path.join(docs, "README.md");
  const lesson = path.join(docs, "learn", "intro.md");
  assert.equal(repoLinkToDocs("../../CHANGELOG.md", readme, docs), "CHANGELOG.md");
  assert.equal(repoLinkToDocs("../../CHANGELOG.md#201", readme, docs), "CHANGELOG.md#201");
  assert.equal(repoLinkToDocs("../../../CHANGELOG.md", lesson, docs), "../CHANGELOG.md");
  for (const kept of ["upgrading.md", "#top", "https://x.dev/CHANGELOG.md", "../../README.md", "CHANGELOG.md"]) {
    assert.equal(repoLinkToDocs(kept, readme, docs), kept);
  }
});
