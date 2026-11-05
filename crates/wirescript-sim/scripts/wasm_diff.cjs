// Runs every program and `@test fn` that `wirescript test` would run, once
// through the native CLI and once through the wasm build, and compares the
// chat lines and whether each run stopped with an error. Exit 0 means 0 diffs.
//
//   node crates/wirescript-sim/scripts/wasm_diff.cjs <wasm-pkg-dir> <wirescript-exe> <dir>...
const { execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const [pkgDir, exe, ...dirs] = process.argv.slice(2);
if (!pkgDir || !exe || dirs.length === 0) {
  console.error("usage: wasm_diff.cjs <wasm-pkg-dir> <wirescript-exe> <dir>...");
  process.exit(2);
}
const wasm = require(path.resolve(pkgDir, "wasm.js"));
const TICKS = 600;

// The text `sim` prints for a chat line: control characters escaped.
const hidden = (c) =>
  c <= 0x1f || (c >= 0x7f && c <= 0x9f) || (c >= 0x200b && c <= 0x200f) || (c >= 0x2028 && c <= 0x202e) ||
  (c >= 0x2060 && c <= 0x2069) || c === 0xfeff;
const escape = (s) =>
  [...s].map((ch) => (hidden(ch.codePointAt(0)) ? "\\u{" + ch.codePointAt(0).toString(16) + "}" : ch)).join("");

function native(file, test, script) {
  const args = ["sim", file, "--ticks", String(TICKS)];
  if (script) args.push("--script", script);
  if (test) args.push("--test", test);
  try {
    const out = execFileSync(exe, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    return { lines: out.split("\n").filter((l, i, a) => i < a.length - 1 || l !== "").map((l) => l.replace(/\r$/, "")), error: false };
  } catch (e) {
    const out = (e.stdout || "").split("\n").filter((l) => l !== "").map((l) => l.replace(/\r$/, ""));
    return { lines: out, error: true };
  }
}

function inWasm(file, test, script) {
  const src = fs.readFileSync(file, "utf8");
  const scriptText = script ? fs.readFileSync(script, "utf8") : undefined;
  const r = JSON.parse(wasm.wirescript_simulate(src, file, undefined, undefined, TICKS, 1 / 60, undefined, test, scriptText));
  return { lines: r.lines.map((l) => escape(l.text)), error: r.error !== null };
}

let runs = 0;
let diffs = 0;
for (const dir of dirs) {
  for (const name of fs.readdirSync(dir).sort()) {
    if (!name.endsWith(".ws")) continue;
    const file = path.join(dir, name);
    const stem = file.slice(0, -3);
    const script = fs.existsSync(stem + ".script") ? stem + ".script" : undefined;
    const src = fs.readFileSync(file, "utf8");
    const tests = [...src.matchAll(/^@test\s+(?:fn|mod|chip)\s+([A-Za-z_][A-Za-z0-9_]*)/gm)].map((m) => m[1]);
    const cases = [];
    if (fs.existsSync(stem + ".expected")) cases.push(undefined);
    cases.push(...tests);
    for (const test of cases) {
      runs++;
      const label = test ? `${file}::${test}` : file;
      const a = native(file, test, script);
      const b = inWasm(file, test, script);
      const same = a.error === b.error && a.lines.length === b.lines.length && a.lines.every((l, i) => l === b.lines[i]);
      if (same) {
        console.log(`same ${label} (${a.lines.length} lines)`);
      } else {
        diffs++;
        console.log(`DIFF ${label}`);
        console.log(`  native${a.error ? " (error)" : ""}: ${JSON.stringify(a.lines)}`);
        console.log(`  wasm${b.error ? " (error)" : ""}:   ${JSON.stringify(b.lines)}`);
      }
    }
  }
}
console.log(`${runs} runs, ${diffs === 0 ? "0 differences" : `${diffs} differ`}`);
process.exit(diffs === 0 && runs > 0 ? 0 : 1);
