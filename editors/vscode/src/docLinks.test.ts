import { test } from "node:test";
import * as assert from "node:assert/strict";
import { bundleDocLinks, bundledPage, handlerUri, openPageCommand } from "./docLinks";

const docs = "https://wirescript.brickadia.dev/docs/";

test("a docs URL names a bundled page and section", () => {
  assert.deepEqual(bundledPage(`${docs}builtins.html#setlocation`), ["builtins.md", "setlocation"]);
  assert.deepEqual(bundledPage(`${docs}learn/tools.html`), ["learn/tools.md"]);
  assert.deepEqual(bundledPage(`${docs}index.html`), ["README.md"]);
  assert.deepEqual(bundledPage(`${docs}learn/index.html#a`), ["learn/README.md", "a"]);
  assert.equal(bundledPage("https://example.com/docs/builtins.html"), undefined);
  assert.equal(bundledPage(`${docs}images/logo.png`), undefined);
});

test("hover links open the bundled page", () => {
  const command = openPageCommand(`${docs}types.html#primitive-types`);
  assert.equal(command, `command:wirescript.openDocsPage?${encodeURIComponent('["types.md","primitive-types"]')}`);
  const hover = "```wirescript\nint\n```\n\n[Docs](" + docs + "types.html#primitive-types)";
  assert.equal(bundleDocLinks(hover), "```wirescript\nint\n```\n\n[Docs](" + command + ")");
  const other = "[site](https://example.com/a.html)";
  assert.equal(bundleDocLinks(other), other);
});

test("diagnostic links go through the URI handler", () => {
  const base = "vscode://publisher.wirescript";
  assert.equal(handlerUri(`${docs}diagnostics.html#types--operators`, base), `${base}/docs?page=diagnostics.md#types--operators`);
  assert.equal(handlerUri(`${docs}learn/tools.html`, base), `${base}/docs?page=learn%2Ftools.md`);
  assert.equal(handlerUri("https://example.com/", base), undefined);
});
