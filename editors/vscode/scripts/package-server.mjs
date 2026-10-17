// Put the WASI language server in `server/`, where the packaged extension
// looks for it, shrunk with `wasm-opt -O3` when that is installed (about 20%
// smaller, no measurable speed change). Run by `just vscode-package` after
// `just lsp-wasi`.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ext = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const built = join(ext, "..", "..", "target", "wasm32-wasip1-threads", "release", "wirescript-lsp.wasm");
const out = join(ext, "server", "wirescript-lsp.wasm");
if (!existsSync(built)) {
  console.error(`package-server: no ${built}; run \`just lsp-wasi\` first`);
  process.exit(1);
}
mkdirSync(dirname(out), { recursive: true });
const opt = spawnSync("wasm-opt", ["-O3", built, "-o", out], { stdio: "inherit" });
if (opt.error || opt.status !== 0) {
  console.warn("package-server: wasm-opt failed or is not installed; shipping the unoptimized build");
  copyFileSync(built, out);
}
const mb = (p) => (statSync(p).size / 1e6).toFixed(1);
console.log(`package-server: ${out} (${mb(out)} MB, built ${mb(built)} MB)`);
