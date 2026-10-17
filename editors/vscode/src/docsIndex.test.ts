import { test } from "node:test";
import * as assert from "node:assert/strict";
import { flattenPages, parseSummary } from "./docsIndex";

const SUMMARY = `<!-- Generated. Do not edit. -->

[Overview](README.md)

- [Learn](learn/README.md)
  - [Basics]()
    - [Tools](learn/tools.md)
    - [First Circuit](learn/first-circuit.md)
  - [State]()
    - [Variables](learn/state/variables.md)
- [Syntax](syntax.md)
`;

test("nests entries by indentation", () => {
  const roots = parseSummary(SUMMARY);
  assert.deepEqual(roots.map((n) => n.title), ["Overview", "Learn", "Syntax"]);
  const learn = roots[1];
  assert.deepEqual(learn.children.map((n) => n.title), ["Basics", "State"]);
  assert.equal(learn.children[0].path, undefined);
  assert.deepEqual(learn.children[0].children.map((n) => n.path), ["learn/tools.md", "learn/first-circuit.md"]);
});

test("flattens pages with the headings above them", () => {
  const pages = flattenPages(parseSummary(SUMMARY));
  assert.deepEqual(pages.map((p) => p.path), [
    "README.md",
    "learn/README.md",
    "learn/tools.md",
    "learn/first-circuit.md",
    "learn/state/variables.md",
    "syntax.md",
  ]);
  assert.deepEqual(pages.find((p) => p.path === "learn/state/variables.md")?.trail, ["Learn", "State"]);
});
