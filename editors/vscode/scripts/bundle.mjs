// Bundle the extension into `dist/extension.js`, the file the package runs,
// with the Oniguruma wasm the markdown preview's highlighter loads beside it.
// `tsc` (in `npm run build`) still type-checks and builds `out/` for the tests.
import { build } from "esbuild";
import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ext = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(ext, "dist");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

await build({
  entryPoints: [join(ext, "src", "extension.ts")],
  outfile: join(dist, "extension.js"),
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  // The unbundled fallback in loadGrammar; the package always has dist/onig.wasm.
  external: ["vscode", "vscode-oniguruma/release/onig.wasm"],
  minify: true,
  sourcemap: true,
  logLevel: "warning",
});

const require = createRequire(import.meta.url);
copyFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"), join(dist, "onig.wasm"));

// The runner ("Wirescript: Play"): the playground's view and loop, the
// webview's own page, and the simulation worker over the Node build of the
// wasm crate (`just wasm` builds it into the playground's sdk/pkg).
const runner = join(dist, "runner");
const playground = join(ext, "..", "..", "crates", "wasm", "playground");
const pkg = join(playground, "sdk", "pkg");
if (!existsSync(join(pkg, "wasm.js"))) {
  throw new Error(`bundle: ${pkg} is missing; run "just wasm" first`);
}
mkdirSync(runner, { recursive: true });
for (const f of ["runner-view.js", "runner-core.js", "runner.css"]) copyFileSync(join(playground, f), join(runner, f));
for (const f of ["webview.js", "page.css"]) copyFileSync(join(ext, "runner", f), join(runner, f));
for (const f of ["wasm.js", "wasm_bg.wasm"]) copyFileSync(join(pkg, f), join(runner, f));
await build({
  entryPoints: [join(ext, "runner", "worker.mjs")],
  outfile: join(runner, "worker.js"),
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  external: ["./wasm.js"],
  minify: true,
  logLevel: "warning",
});
console.log(`bundle: ${join(dist, "extension.js")}`);
