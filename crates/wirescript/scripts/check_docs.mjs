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
//   ```wirescript expect=WSP001 -> under learn/, skipped here: a diagnostic
//                               shown on purpose, which check_learn.mjs
//                               requires to report that code
// A block may also carry a hidden prelude: an HTML comment
//   <!-- doc-check-prelude:
//   <ws lines...>
//   -->
// immediately before the fence is PREPENDED to the block before checking but is
// NOT rendered — use it to give a fragment just enough context (a `var`, an `in`)
// without cluttering the shown example. Multiple preludes accumulate until a
// fenced block consumes them.

import { readFileSync, writeFileSync, mkdtempSync, existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import { binPath, filesUnder, repoRoot, wsBlocks } from "./lib.mjs";

const docsDir = join(repoRoot, "docs", "src");
const checkBin = binPath("wirescript-check");

// Without this the gate reports "N checked, 0 failed" and exits 0 when the
// checker was never built, which reads exactly like a pass.
if (!existsSync(checkBin)) {
  console.error(`doc-check: ${checkBin} does not exist -- run \`just check-bin\` first`);
  process.exit(1);
}

const scratch = mkdtempSync(join(tmpdir(), "ws-doccheck-"));

let checked = 0;
let skipped = 0;
const failures = [];

const mdFiles = filesUnder(docsDir, ".md");
for (const md of mdFiles) {
  const text = readFileSync(join(docsDir, md), "utf8");
  const blocks = wsBlocks(text);
  blocks.forEach((b, idx) => {
    // An `expect=` block shows an error on purpose; check_learn.mjs checks
    // that it fails with that code.
    if (b.ignore || (md.startsWith("learn/") && b.words.some((w) => w.startsWith("expect=")))) {
      skipped++;
      return;
    }
    checked++;
    const src = (b.prelude ? b.prelude : "") + b.code + "\n";
    const file = join(scratch, `${md.replaceAll("/", "_")}.${idx}.ws`);
    writeFileSync(file, src);
    try {
      execFileSync(checkBin, [file], { stdio: "pipe" });
    } catch (e) {
      // `wirescript-check` writes a raw colour escape before the label, so
      // `\bERROR\b` can never match: the character before `E` is the `m` that
      // ends the escape, which is a word character, so there is no boundary.
      // Strip colour before any of these tests, or the gate below silently
      // passes every block.
      const out = `${e.stdout ?? ""}${e.stderr ?? ""}`
        .toString()
        .replace(/\x1b\[[0-9;]*m/g, "");
      const errLines = out.split("\n").filter((l) => /ERROR|WS\d|WSP\d/.test(l));
      // Parse-focused gate: a block fails ONLY on a real PARSE error (WSP*) —
      // that catches syntax rot (old handler forms, no-parens events, removed
      // keywords). SEMANTIC errors are tolerated: a doc snippet is often an
      // illustrative fragment that references names defined elsewhere (WS002),
      // uses a placeholder trigger (WS001), or shows an isolated exec statement
      // (WS007). The `...`/`…` ellipsis (the docs' "elided code" convention) is
      // also tolerated. A genuinely un-parseable fragment can opt out with
      // ` ```wirescript ignore`.
      //
      // A parse error is Error-severity (label `ERROR`); the WSP001 the LOWERING
      // stage emits for an `_Unsupported` placeholder is a WARN, and is tolerated
      // like the other semantic issues: a fragment referencing an undefined name
      // lowers its use to a placeholder, which is not syntax rot.
      const realParseErrs = errLines.filter(
        (l) =>
          /WSP\d/.test(l) &&
          /\bERROR\b/.test(l) &&
          !/unexpected token '(\.\.\.|…)'/.test(l),
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
