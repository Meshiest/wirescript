#!/usr/bin/env node
// Regenerate data/logic_gate_inventory.simple.json — the gate catalog the
// compiler bakes in via include_str! — from the in-game inventory dump.
//
// Sources (merge):
//   1. data/inventory_dump.ndjson  — the rich Lua dump (lua/inventory_dump.lua
//      in re4ss-mcp): brick/component metadata, display names, categories, and
//      per-port UE property types + display names. This is the primary source.
//   2. data/inventory_brdb.json    — authoritative wire-port lists + component
//      schemas extracted from a placed-components .brdb save (brdb example
//      `dump_inventory`). Used to VALIDATE that the ndjson port discovery is
//      complete; not required to run.
//   3. the existing simple.json    — preserves hand-curated wire-port types
//      (int/float/exec/varRef/...) that the dump can't express, since every
//      wire-graph variant port reports merely as `StructProperty`.
//
// Merge policy per port:
//   * curated non-`any` type from the existing simple.json always wins (keeps
//     the precise wire-graph typing the dump lacks);
//   * otherwise a concrete UE scalar type from the dump is used (this enriches
//     physical-brick props that used to be `any`);
//   * otherwise a name/heuristic fallback (exec/varRef/index/object/...).
//
// kind/family are derived from the class name; for classes already present in
// the existing simple.json the curated family is reused (no regressions). The
// script is idempotent: re-running against its own output is a no-op.
//
//   node scripts/gen_inventory.mjs            # regenerate in place
//   node scripts/gen_inventory.mjs --check    # don't write; report diff only
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DATA = join(HERE, "..", "data");
const NDJSON = join(DATA, "inventory_dump.ndjson");
const BRDB = join(DATA, "inventory_brdb.json");
const OUT = join(DATA, "logic_gate_inventory.simple.json");

const checkOnly = process.argv.includes("--check");

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

// UE property type -> simple scalar type (null = not a concrete scalar).
function ueScalar(ue) {
  switch (ue) {
    case "BoolProperty":
      return "bool";
    case "FloatProperty":
    case "DoubleProperty":
      return "float";
    case "IntProperty":
    case "Int64Property":
    case "ByteProperty":
    case "EnumProperty":
      return "int";
    case "StrProperty":
    case "NameProperty":
    case "TextProperty":
      return "string";
    default:
      return null; // StructProperty / Object / Class / WeakObject
  }
}

// Index-ish wire ports the dump reports as opaque variants but are really ints.
const INT_PORT_NAMES = new Set([
  "Index", "IndexA", "IndexB", "Start", "Count", "Size", "Length", "Capacity",
]);

// Name/heuristic fallback for ports the dump leaves as StructProperty / object.
function heuristicType(name, displayName, ue) {
  if (name === "Exec" || name === "ExecOut" || /^Exec/.test(name)) return "exec";
  if (name === "ArrayVarRef" || name.endsWith("ArrayVarRef")) return "arrayVarRef";
  if (name.endsWith("VarRef")) return "varRef";
  if (INT_PORT_NAMES.has(name)) return "int";
  if (/^b[A-Z]/.test(name)) return "bool";
  if (ue === "WeakObjectProperty") {
    if (/Character/.test(name)) return "character";
    if (/Controller/.test(name)) return "controller";
    return "entity";
  }
  return "any";
}

// Composite kind from the set of `.sub` suffixes under a struct port.
function compositeKind(subs) {
  const s = new Set(subs);
  const eq = (...keys) => keys.length === s.size && keys.every((k) => s.has(k));
  if (eq("X", "Y", "Z")) return "vector";
  if (eq("Pitch", "Yaw", "Roll")) return "rotator";
  if (eq("R", "G", "B", "A")) return "color";
  return "struct";
}

const COMPOSITE_TYPE = { vector: "vector", rotator: "rotator", color: "color", struct: "any" };

// ---------------------------------------------------------------------------
// kind / family derivation
// ---------------------------------------------------------------------------

function deriveKind(cls) {
  if (/Pseudo/.test(cls)) return "pseudo";
  if (/_Expr_/.test(cls)) return "expr";
  if (/_Exec_/.test(cls)) return "exec";
  if (/_Fake_/.test(cls)) return "fake";
  if (/_Internal_|^Component_Internal/.test(cls)) return "internal";
  return "?";
}

function deriveFamily(cls) {
  // Exec gates: family is the subsystem segment, e.g. _Exec_Entity_* -> Entity.
  let m = cls.match(/_Exec_([A-Za-z0-9]+)_/);
  if (m) return m[1];
  m = cls.match(/WireGraphPseudo_([A-Za-z0-9]+)$/);
  if (m) return m[1];
  m = cls.match(/_Expr_([A-Za-z0-9]+)/);
  if (m) {
    const x = m[1];
    if (x.startsWith("Math")) return "Math";
    if (x.startsWith("Compare") || x === "NearlyEqual") return "Compare";
    if (x.startsWith("String")) return "String";
    if (x.startsWith("Logical")) return "Logical";
    if (x.startsWith("Bitwise")) return "Bitwise";
    if (x === "MakeVector" || x === "SplitVector") return x;
    if (x === "MakeColor" || x === "SplitColor") return x;
    if (x.startsWith("Vec")) return "Vec";
    if (/^(Select|Swap|EdgeDetector|Branch)$/.test(x)) return x;
    return x;
  }
  m = cls.match(/_(CharacterZoneEvent|ZoneEvent)_/);
  if (m) return m[1];
  m = cls.match(/_Internal_([A-Za-z0-9]+)/);
  if (m) return m[1];
  return "";
}

// ---------------------------------------------------------------------------
// Port building
// ---------------------------------------------------------------------------

// Turn a {portName: {type, tooltip, displayName}} dict into the simple port
// list: collapse `name.sub` composites, resolve types via the merge policy.
function buildPorts(portDict, curatedByName) {
  // Group composite sub-ports under their parent.
  const subgroups = new Map(); // parent -> Set(suffix)
  for (const pn of Object.keys(portDict)) {
    if (pn.includes(".")) {
      const [base, sub] = [pn.slice(0, pn.indexOf(".")), pn.slice(pn.indexOf(".") + 1)];
      if (!subgroups.has(base)) subgroups.set(base, new Set());
      subgroups.get(base).add(sub.split(".")[0]);
    }
  }

  const out = [];
  for (const [name, meta] of Object.entries(portDict)) {
    if (name.includes(".")) continue; // sub-port, folded into its parent
    const curated = curatedByName.get(name);
    const ue = meta.type;
    const displayName = meta.displayName || curated?.displayName || name;
    const tooltip = (meta.tooltip && meta.tooltip.length ? meta.tooltip : curated?.tooltip) || "";

    let type;
    let composite = null;
    if (subgroups.has(name)) {
      const subs = [...subgroups.get(name)].sort();
      const kind = compositeKind(subs);
      type = COMPOSITE_TYPE[kind];
      composite = { kind, subPorts: subs };
      // Prefer the curated composite shape if one already existed.
      if (curated?.composite) {
        composite = curated.composite;
        type = curated.type;
      }
    } else {
      const curatedType = curated?.type;
      const scalar = ueScalar(ue);
      if (curatedType && curatedType !== "any") {
        type = curatedType; // curated precision wins
      } else if (scalar) {
        type = scalar; // enrich from the dump's concrete UE type
      } else {
        type = heuristicType(name, displayName, ue);
      }
      composite = curated?.composite ?? null;
    }

    out.push({ name, displayName, tooltip, type, composite });
  }
  out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function loadNdjson(path) {
  const recs = [];
  for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    if (line.trim().length) recs.push(JSON.parse(line));
  }
  // de-dupe by assetName, last write wins (matches merge_dump.mjs)
  const byName = new Map();
  for (const r of recs) byName.set(r.assetName, r);
  return [...byName.values()];
}

const existing = JSON.parse(readFileSync(OUT, "utf8"));
const existingByClass = new Map(existing.entries.map((e) => [e.component.class, e]));

const records = loadNdjson(NDJSON);

const entries = [];
for (const r of records) {
  const comp = r.components?.[0];
  if (!comp) continue;
  const cls = comp.componentType;
  const cur = existingByClass.get(cls);
  const curIn = new Map((cur?.component.inputs ?? []).map((p) => [p.name, p]));
  const curOut = new Map((cur?.component.outputs ?? []).map((p) => [p.name, p]));

  entries.push({
    brickAsset: r.assetName,
    brickDisplayName: r.brickDisplayName ?? "",
    brickSummary: r.brickSummary ?? "",
    halfSize: { X: r.halfSize?.X ?? 0, Y: r.halfSize?.Y ?? 0, Z: r.halfSize?.Z ?? 0 },
    component: {
      class: cls,
      displayName: comp.componentDisplayName ?? "",
      description: comp.componentDescription ?? "",
      kind: deriveKind(cls),
      // Reuse curated family for known classes; derive for new ones.
      family: cur ? cur.component.family : deriveFamily(cls),
      inputs: buildPorts(comp.wireInputPorts ?? {}, curIn),
      outputs: buildPorts(comp.wireOutputPorts ?? {}, curOut),
    },
  });
}
// Carry over curated entries whose class is absent from this dump snapshot.
// The compiler references some gates (e.g. Gamemode_EndRound) that a given
// components save may not contain; dropping them would break those builtins.
const dumpedClasses = new Set(entries.map((e) => e.component.class));
const carried = [];
for (const e of existing.entries) {
  if (!dumpedClasses.has(e.component.class)) {
    entries.push(e);
    carried.push(e.component.class);
  }
}

entries.sort((a, b) => a.brickAsset.localeCompare(b.brickAsset));

// --- validation against brdb (authoritative port lists), if present ----------
let brdbWarnings = 0;
try {
  const brdb = JSON.parse(readFileSync(BRDB, "utf8"));
  const bByClass = new Map(brdb.components.map((c) => [c.class, c]));
  const strip = (p) => p.split(".")[0];
  for (const e of entries) {
    const b = bByClass.get(e.component.class);
    if (!b) continue;
    const want = (arr) => new Set(arr.map(strip));
    const got = (ports) => new Set(ports.map((p) => p.name));
    for (const [dir, bports, eports] of [
      ["in", b.inputs, e.component.inputs],
      ["out", b.outputs, e.component.outputs],
    ]) {
      const w = want(bports), g = got(eports);
      const missing = [...w].filter((p) => !g.has(p));
      if (missing.length) {
        brdbWarnings++;
        console.warn(`  ! ${e.component.class} ${dir}: ndjson missing brdb ports ${missing.join(", ")}`);
      }
    }
  }
} catch (err) {
  if (err.code !== "ENOENT") throw err;
}

const out = { entries, typeGlossary: existing.typeGlossary ?? {} };

// --- diff summary vs current file -------------------------------------------
const newByClass = new Map(entries.map((e) => [e.component.class, e]));
const added = [...newByClass.keys()].filter((c) => !existingByClass.has(c));
const removed = [...existingByClass.keys()].filter((c) => !newByClass.has(c));
let typeChanges = 0;
for (const [cls, e] of newByClass) {
  const cur = existingByClass.get(cls);
  if (!cur) continue;
  const curp = new Map([...cur.component.inputs, ...cur.component.outputs].map((p) => [p.name, p.type]));
  for (const p of [...e.component.inputs, ...e.component.outputs]) {
    if (curp.has(p.name) && curp.get(p.name) !== p.type) typeChanges++;
  }
}

console.log(`entries: ${existing.entries.length} -> ${entries.length}`);
console.log(`new classes: ${added.length}, removed: ${removed.length}, port type changes: ${typeChanges}`);
if (removed.length) console.log(`  removed: ${removed.map((c) => c.replace("BrickComponentType_", "")).join(", ")}`);
if (carried.length) console.log(`  carried over (absent from dump, kept from curated): ${carried.map((c) => c.replace("BrickComponentType_", "")).join(", ")}`);
if (brdbWarnings) console.log(`  ${brdbWarnings} brdb port-coverage warnings (see above)`);

if (checkOnly) {
  console.log("--check: not writing");
} else {
  writeFileSync(OUT, JSON.stringify(out, null, 2) + "\n");
  console.log(`wrote ${OUT}`);
}
