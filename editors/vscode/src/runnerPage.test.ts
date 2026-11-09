import { test } from "node:test";
import * as assert from "node:assert/strict";
import { moduleName, runnerHtml, siblingFiles, viewMessage } from "./runnerPage";

const ASSETS = {
  cspSource: "https://ext.example",
  glue: "https://ext.example/webview.js",
  css: "https://ext.example/runner.css",
  pageCss: "https://ext.example/page.css",
};

test("the page loads scripts and styles from the extension only", () => {
  const html = runnerHtml(ASSETS, "Play: a.ws");
  assert.match(html, /content="default-src 'none'; base-uri 'none'; form-action 'none'; script-src https:\/\/ext\.example; style-src https:\/\/ext\.example"/);
  assert.match(html, /<script type="module" src="https:\/\/ext\.example\/webview\.js"><\/script>/);
  assert.doesNotMatch(html, /<script>|<style>|unsafe-/, "no inline script or style");
});

test("the title is escaped", () => {
  assert.match(runnerHtml(ASSETS, "Play: <b>.ws"), /<title>Play: &#60;b&#62;\.ws<\/title>/);
});

test("the page may sit, stand and hold inputs as the local player", () => {
  for (const line of ["sit you p1", "stand you", "input you Forward -1", "input you E 1", "input you Right 0.5"]) {
    assert.deepEqual(viewMessage({ type: "line", line }), { type: "line", line });
  }
  assert.deepEqual(viewMessage({ type: "ready" }), { type: "ready" });
});

test("anything else from the page is dropped", () => {
  for (const msg of [
    null,
    "sit you p1",
    { type: "start", source: "x" },
    { type: "line", line: "chat you /kick bob" },
    { type: "line", line: "sit bob p1" },
    { type: "line", line: "sit you p1\n1 join eve" },
    { type: "line", line: "input you Forward 1e9" },
    { type: "line", line: 3 },
    { type: "line", line: `sit you ${"a".repeat(200)}` },
  ]) {
    assert.equal(viewMessage(msg), undefined, JSON.stringify(msg));
  }
});

test("imports read the other .ws files beside the program", () => {
  const files = siblingFiles(
    [
      { name: "main.ws", text: "main" },
      { name: "lib.ws", text: "lib" },
      { name: "notes.md", text: "notes" },
    ],
    "main.ws",
  );
  assert.deepEqual(JSON.parse(files), { "lib.ws": "lib" });
  assert.equal(moduleName("snake.ws"), "snake");
});
