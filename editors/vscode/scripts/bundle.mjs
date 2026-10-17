// Bundle the extension into `dist/extension.js`, the file the package runs,
// with the Oniguruma wasm the markdown preview's highlighter loads beside it.
// `tsc` (in `npm run build`) still type-checks and builds `out/` for the tests.
import { build } from "esbuild";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
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
console.log(`bundle: ${join(dist, "extension.js")}`);
