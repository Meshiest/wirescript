#!/usr/bin/env node
// Regenerate data/asset_inventory.simple.json — the external-asset catalog the compiler
// bakes in via include_str! — from the in-game catalog dump.
//
// Source: data/asset_dump.json, a copy of brickadia-zoo's out/catalog_dump.json
// (produced by lua/catalog_dump.lua). Copy it in the same way as inventory_dump.ndjson:
//   cp ../../../brickadia-zoo/out/catalog_dump.json data/asset_dump.json
//
// Why this exists: wirescript writes external asset refs as `$<DescriptorType>/<AssetName>`
// (e.g. `$BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press`), but nothing told the
// compiler which names are real — a typo'd or removed asset compiled fine and only failed
// in-game. The gate inventory carries the `Asset` config slot but types it merely `object`.
//
// The dump keys its types by a short label ("OneShotAudio"); the name that matters here is
// the PRIMARY ASSET TYPE, which is what appears after the `$` in source. It is carried in the
// dump's `source` field as "primary:<Type>".
//
// Each asset also carries the game's FBRCatalogData — the Tab/Category/DisplayName the in-game
// picker groups by. Keep it: matching assets to their owning weapon/vehicle by NAME is wrong
// (category "Typewriter" vs item "Weapon_TypewriterSMG"), the category is the real relation.
//
//   node scripts/gen_assets.mjs            # regenerate in place
//   node scripts/gen_assets.mjs --check    # don't write; report the diff only
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DATA = join(HERE, "..", "data");
const DUMP = join(DATA, "asset_dump.json");
const OUT = join(DATA, "asset_inventory.simple.json");

const checkOnly = process.argv.includes("--check");

if (!existsSync(DUMP)) {
  console.error(`missing ${DUMP}
Copy it from the brickadia-zoo dump first:
  cp <brickadia-zoo>/out/catalog_dump.json crates/wirescript/data/asset_dump.json`);
  process.exit(1);
}

const dump = JSON.parse(readFileSync(DUMP, "utf8"));
const existing = existsSync(OUT) ? JSON.parse(readFileSync(OUT, "utf8")) : { assetTypes: [] };

// "primary:BrickOneShotAudioDescriptor" -> "BrickOneShotAudioDescriptor"
const primaryTypeOf = (src) => (typeof src === "string" && src.startsWith("primary:") ? src.slice(8) : null);

const assetTypes = [];
let totalAssets = 0;
let missingMeta = 0;

for (const [key, group] of Object.entries(dump.assets ?? {})) {
  const type = primaryTypeOf(group.source);
  if (!type) {
    console.log(`  ! skipping ${key}: source "${group.source}" is not a primary-asset type`);
    continue;
  }
  // `entries` carries the catalog metadata; fall back to the bare `names` list for a dump
  // produced before catalog_dump.lua started emitting it.
  const rows = Array.isArray(group.entries) && group.entries.length
    ? group.entries
    : (group.names ?? []).map((name) => ({ name }));

  const assets = rows.map((e) => {
    const c = e.catalog ?? {};
    const a = { name: e.name };
    if (c.DisplayName) a.displayName = c.DisplayName;
    if (c.Tab) a.tab = c.Tab;
    if (c.Category) a.category = c.Category;
    if (c.Summary) a.summary = c.Summary;
    if (c.SearchTags) a.searchTags = c.SearchTags;
    // Only emit the flags when they deviate from the norm, to keep the file readable.
    if (c.bAdvanced === true) a.advanced = true;
    if (c.bShouldDisplay === false) a.hidden = true;
    if (!e.catalog) missingMeta++;
    return a;
  });
  assets.sort((x, y) => (x.name < y.name ? -1 : x.name > y.name ? 1 : 0));

  totalAssets += assets.length;
  assetTypes.push({ type, key, count: assets.length, assets });
}
assetTypes.sort((a, b) => (a.type < b.type ? -1 : a.type > b.type ? 1 : 0));

const out = {
  // How a ref is written in source: $<type>/<asset name>.
  refSyntax: "$<type>/<name>",
  assetTypes,
};

// --- report ------------------------------------------------------------------
const prevByType = new Map((existing.assetTypes ?? []).map((t) => [t.type, t]));
const nowByType = new Map(assetTypes.map((t) => [t.type, t]));
console.log(`asset types: ${prevByType.size} -> ${nowByType.size}, assets: ${totalAssets}`);
for (const [type, t] of nowByType) {
  const prev = prevByType.get(type);
  if (!prev) {
    console.log(`  + ${type}: ${t.count} (new type)`);
    continue;
  }
  const before = new Set(prev.assets.map((a) => a.name));
  const after = new Set(t.assets.map((a) => a.name));
  const added = [...after].filter((n) => !before.has(n));
  const removed = [...before].filter((n) => !after.has(n));
  if (added.length || removed.length) {
    console.log(`  ~ ${type}: ${prev.count} -> ${t.count}` +
      (added.length ? `  +${added.length} (${added.slice(0, 6).join(", ")}${added.length > 6 ? ", …" : ""})` : "") +
      (removed.length ? `  -${removed.length} (${removed.slice(0, 6).join(", ")}${removed.length > 6 ? ", …" : ""})` : ""));
  } else {
    console.log(`  = ${type}: ${t.count}`);
  }
}
for (const type of prevByType.keys()) if (!nowByType.has(type)) console.log(`  - ${type}: REMOVED`);
if (missingMeta) console.log(`  ${missingMeta} asset(s) without catalog metadata (not resident at dump time)`);

if (checkOnly) {
  console.log("--check: not writing");
} else {
  writeFileSync(OUT, JSON.stringify(out, null, 2) + "\n");
  console.log(`wrote ${OUT}`);
}
