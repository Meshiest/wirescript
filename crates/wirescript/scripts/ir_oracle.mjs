#!/usr/bin/env node
// Byte-for-byte output oracle for the wirescript-ir refactor.
//
// Compiles every `.ws` under `<dir>/src` and compares each `.brz` with the
// baseline built before the refactor started. Compiler output is deterministic
// across processes (checked 2026-10-12, source-prefab programs included), so
// the compare is on raw bytes.
//
//   node ir_oracle.mjs baseline [--bin <exe>]   build <dir>/baseline
//   node ir_oracle.mjs check    [--bin <exe>]   compile into <dir>/current, compare
//   node ir_oracle.mjs check --via-ir           compile --emit-ir, then assemble (which
//                                              also checks the text is a fixed point)
//
// `<dir>` defaults to `target/ir-oracle` (override with --dir). Its `src/` is a
// frozen snapshot of the oracle set: the wirescript tests/fixtures, the repo
// examples, and every `wirescript/projects/*/src`. Files that fail to compile
// at baseline must still fail; their failures are not regressions.
//
// Exit code 0 means 0 differences.

import {
  readFileSync,
  writeFileSync,
  readdirSync,
  mkdirSync,
  existsSync,
  rmSync,
} from "node:fs";
import { join, dirname, relative, resolve as resolvePath } from "node:path";
import { spawn } from "node:child_process";
import { cpus } from "node:os";
import { binPath, opt, repoRoot } from "./lib.mjs";

const args = process.argv.slice(2);
const mode = args[0];
const flag = (name) => args.includes(name);

const dir = resolvePath(opt("--dir", join(repoRoot, "target", "ir-oracle")));
const bin = resolvePath(opt("--bin", binPath("wirescript")));
const viaIr = flag("--via-ir");
const jobs = Number(opt("--jobs", Math.max(1, cpus().length - 2)));
const srcDir = join(dir, "src");

if (mode !== "baseline" && mode !== "check") {
  console.error("usage: ir_oracle.mjs baseline|check [--bin exe] [--dir dir] [--via-ir] [--jobs n]");
  process.exit(2);
}
if (!existsSync(srcDir)) {
  console.error(`no oracle source snapshot at ${srcDir}`);
  process.exit(2);
}

function listWs(root) {
  const out = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith(".ws")) out.push(relative(root, p).split("\\").join("/"));
    }
  };
  walk(root);
  return out.sort();
}

function run(argv) {
  return new Promise((resolve) => {
    const child = spawn(bin, argv, { cwd: srcDir });
    let err = "";
    child.stderr.on("data", (d) => (err += d));
    child.stdout.on("data", (d) => (err += d));
    child.on("close", (code) => resolve({ code, err }));
  });
}

async function compileOne(rel, outRoot) {
  const out = join(outRoot, rel.replace(/\.ws$/, ".brz"));
  mkdirSync(dirname(out), { recursive: true });
  if (existsSync(out)) rmSync(out);
  if (!viaIr) {
    const r = await run(["compile", rel, "-o", out]);
    return { ok: r.code === 0 && existsSync(out), err: lastLine(r.err) };
  }
  const ir = out.replace(/\.brz$/, ".wsir");
  const c = await run(["compile", rel, "--emit-ir", ir, "-o", out.replace(/\.brz$/, ".direct.brz")]);
  if (c.code !== 0) return { ok: false, err: lastLine(c.err) };
  const a = await run(["assemble", ir, "-o", out, "--check-roundtrip"]);
  return { ok: a.code === 0 && existsSync(out), err: lastLine(a.err) };
}

function lastLine(s) {
  const lines = s.trim().split(/\r?\n/);
  return lines[lines.length - 1] ?? "";
}

async function pool(items, f) {
  const results = new Array(items.length);
  let next = 0;
  let done = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      results[i] = await f(items[i]);
      done++;
      if (process.stderr.isTTY) process.stderr.write(`\r${done}/${items.length}`);
    }
  };
  await Promise.all(Array.from({ length: jobs }, worker));
  if (process.stderr.isTTY) process.stderr.write("\n");
  return results;
}

const files = listWs(srcDir);
const t0 = Date.now();

if (mode === "baseline") {
  const outRoot = join(dir, "baseline");
  rmSync(outRoot, { recursive: true, force: true });
  const results = await pool(files, (rel) => compileOne(rel, outRoot));
  const manifest = {};
  files.forEach((rel, i) => (manifest[rel] = results[i]));
  writeFileSync(join(outRoot, "manifest.json"), JSON.stringify(manifest, null, 1));
  const failed = files.filter((rel) => !manifest[rel].ok);
  console.log(`baseline: ${files.length} files, ${files.length - failed.length} compiled, ${failed.length} fail at baseline (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
  process.exit(0);
}

const baseRoot = join(dir, "baseline");
const manifest = JSON.parse(readFileSync(join(baseRoot, "manifest.json"), "utf8"));
const outRoot = join(dir, viaIr ? "current-ir" : "current");
rmSync(outRoot, { recursive: true, force: true });
const results = await pool(files, (rel) => compileOne(rel, outRoot));

const diffs = [];
let same = 0;
let baselineFail = 0;
files.forEach((rel, i) => {
  const base = manifest[rel];
  const cur = results[i];
  if (!base) return diffs.push(`${rel}: not in baseline`);
  if (!base.ok) {
    baselineFail++;
    if (cur.ok) diffs.push(`${rel}: failed at baseline, compiles now`);
    return;
  }
  if (!cur.ok) return diffs.push(`${rel}: compiled at baseline, fails now: ${cur.err}`);
  const brz = rel.replace(/\.ws$/, ".brz");
  const a = readFileSync(join(baseRoot, brz));
  const b = readFileSync(join(outRoot, brz));
  if (Buffer.compare(a, b) !== 0) {
    let at = 0;
    while (at < a.length && at < b.length && a[at] === b[at]) at++;
    return diffs.push(`${rel}: bytes differ (${a.length} vs ${b.length}, first at ${at})`);
  }
  same++;
});
for (const rel of Object.keys(manifest)) {
  if (!files.includes(rel)) diffs.push(`${rel}: missing from the source snapshot`);
}

for (const d of diffs) console.log(`DIFF ${d}`);
console.log(
  `ir-oracle${viaIr ? " (via ir)" : ""}: ${files.length} files, ${same} identical, ${baselineFail} fail at baseline, ${diffs.length} differences (${((Date.now() - t0) / 1000).toFixed(1)}s)`,
);
process.exit(diffs.length === 0 ? 0 : 1);
