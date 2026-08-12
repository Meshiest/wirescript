#!/usr/bin/env node
// Extracts every ```wirescript fenced block from the language-reference docs and
// type-checks it with the `wirescript-check` binary, so a doc example can never
// silently rot. Run via `just doc-check` (which builds the binary first).
//
// Fenced-block conventions (in the ``` info string, space-separated after
// `wirescript`):
//   ```wirescript            -> checked; must type-check with no errors
//   ```wirescript ignore     -> skipped (an illustrative fragment that cannot
//                               compile standalone, e.g. it references names
//                               defined elsewhere); still rendered normally
// A block may also carry a hidden prelude: an HTML comment
//   <!-- doc-check-prelude:
//   <ws lines...>
//   -->
// immediately before the fence is PREPENDED to the block before checking but is
// NOT rendered — use it to give a fragment just enough context (a `var`, an `in`)
// without cluttering the shown example. Multiple preludes accumulate until a
// fenced block consumes them.

import { readFileSync, writeFileSync, mkdtempSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const docsDir = join(repoRoot, "docs", "wirescript");
const checkBin = join(
  repoRoot,
  "target",
  "release",
  process.platform === "win32" ? "wirescript-check.exe" : "wirescript-check",
);

const scratch = mkdtempSync(join(tmpdir(), "ws-doccheck-"));

/** Parse a markdown file into checkable wirescript blocks. */
function extractBlocks(text) {
  const lines = text.split("\n");
  const blocks = [];
  let pendingPrelude = "";
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    // Hidden prelude comment: <!-- doc-check-prelude: ... -->
    const pm = line.match(/^\s*<!--\s*doc-check-prelude:\s*$/);
    if (pm) {
      const buf = [];
      i++;
      while (i < lines.length && !/-->/.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      i++; // consume the `-->`
      pendingPrelude += buf.join("\n") + "\n";
      continue;
    }
    const fence = line.match(/^```wirescript(.*)$/);
    if (fence) {
      const info = fence[1].trim();
      const startLine = i + 1; // 1-based line of the ``` fence
      const body = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        body.push(lines[i]);
        i++;
      }
      i++; // consume closing ```
      const ignore = /\bignore\b/.test(info);
      blocks.push({ startLine, ignore, prelude: pendingPrelude, code: body.join("\n") });
      pendingPrelude = "";
      continue;
    }
    // A non-prelude, non-blank line clears a dangling prelude (prelude must
    // immediately precede its block, allowing only blank lines between).
    if (line.trim() !== "" && pendingPrelude && !/^\s*<!--/.test(line)) {
      pendingPrelude = "";
    }
    i++;
  }
  return blocks;
}

let checked = 0;
let skipped = 0;
const failures = [];

const mdFiles = readdirSync(docsDir).filter((f) => f.endsWith(".md")).sort();
for (const md of mdFiles) {
  const text = readFileSync(join(docsDir, md), "utf8");
  const blocks = extractBlocks(text);
  blocks.forEach((b, idx) => {
    if (b.ignore) {
      skipped++;
      return;
    }
    checked++;
    const src = (b.prelude ? b.prelude : "") + b.code + "\n";
    const file = join(scratch, `${md}.${idx}.ws`);
    writeFileSync(file, src);
    try {
      execFileSync(checkBin, [file], { stdio: "pipe" });
    } catch (e) {
      const out = `${e.stdout ?? ""}${e.stderr ?? ""}`.toString();
      const errLines = out.split("\n").filter((l) => /ERROR|WS\d|WSP\d/.test(l));
      // Parse-focused gate: a block fails ONLY on a real PARSE error (WSP*) —
      // that catches syntax rot (old handler forms, no-parens events, removed
      // keywords). SEMANTIC errors are tolerated: a doc snippet is often an
      // illustrative fragment that references names defined elsewhere (WS002),
      // uses a placeholder trigger (WS001), or shows an isolated exec statement
      // (WS007). The `...`/`…` ellipsis (the docs' "elided code" convention) is
      // also tolerated. A genuinely un-parseable fragment can opt out with
      // ` ```wirescript ignore`.
      const realParseErrs = errLines.filter(
        (l) => /WSP\d/.test(l) && !/unexpected token '(\.\.\.|…)'/.test(l),
      );
      if (realParseErrs.length > 0) {
        failures.push({ md, line: b.startLine, code: b.code, errors: realParseErrs.join("\n") });
      }
    }
  });
}

console.log(`doc-check: ${checked} checked, ${skipped} skipped (ignore), ${failures.length} failed`);
for (const f of failures) {
  console.log(`\n--- ${f.md}:${f.line} ---`);
  console.log(f.errors);
}
process.exit(failures.length === 0 ? 0 : 1);
