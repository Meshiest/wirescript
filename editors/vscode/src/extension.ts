import * as path from "path";
import * as os from "os";
import * as fs from "fs";
import { copyFileToClipboard } from "./clipboard";
import { startWasiServer } from "./wasi";
import type { WasmProcess } from "@vscode/wasm-wasi/v1";
import { translateUri, wasiMounts } from "./wasiPaths";
import { registerDocs } from "./docs";
import { bundleDocLinks, handlerUri, OPEN_DOCS_PAGE } from "./docLinks";
import { loadGrammar, wirescriptMarkdown } from "./markdownPreview";
import {
  workspace,
  ExtensionContext,
  languages,
  commands,
  window,
  env,
  Uri,
  CodeActionKind,
  CodeAction,
  TextDocument,
  Range,
  Position,
  WorkspaceEdit,
  Diagnostic,
  DiagnosticSeverity,
  MarkdownString,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Executable,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
// The temporary copy of the native server binary the running client was started from.
let runningCopy: string | undefined;
// The running WASI server, ended when its client stops.
let wasiProcess: WasmProcess | undefined;
// A WASI server that stops more than MAX_CRASHES times within CRASH_WINDOW_MS
// is left stopped.
const MAX_CRASHES = 4;
const CRASH_WINDOW_MS = 3 * 60 * 1000;

/** Stop the client and delete the copy of the server it ran from. */
async function stopClient(): Promise<void> {
  const stopping = client;
  client = undefined;
  if (stopping) {
    try {
      await stopping.stop();
    } catch {}
  }
  const ending = wasiProcess;
  wasiProcess = undefined;
  if (ending) {
    try {
      await ending.terminate();
    } catch {}
  }
  if (runningCopy) {
    try {
      fs.unlinkSync(runningCopy);
    } catch {}
    runningCopy = undefined;
  }
}

/** Which language server to run, and from where. */
type ServerChoice = { kind: "native"; binary: string } | { kind: "wasi"; wasm: string };

/**
 * `wirescript.server` picks the server. `native` runs `wirescript.lspPath` or
 * this checkout's release build. `wasi` runs `wirescript.wasmPath`, else this
 * checkout's `wasm32-wasip1-threads` build, else the one bundled in `server/`.
 * `auto` takes native when one is configured or built here, WASI otherwise.
 * Returns why nothing can run when there is no such server.
 */
function chooseServer(extDir: string): ServerChoice | string {
  const config = workspace.getConfiguration("wirescript");
  const mode = config.get<string>("server", "auto");
  const repoRoot = path.resolve(extDir, "..", "..");
  const lspPath = config.get<string>("lspPath", "");
  const binary =
    lspPath ||
    path.join(
      repoRoot,
      "target",
      "release",
      `wirescript-lsp${process.platform === "win32" ? ".exe" : ""}`,
    );
  const wasmPath = config.get<string>("wasmPath", "");
  const wasmCandidates = wasmPath
    ? [wasmPath]
    : [
        path.join(repoRoot, "target", "wasm32-wasip1-threads", "release", "wirescript-lsp.wasm"),
        path.join(extDir, "server", "wirescript-lsp.wasm"),
      ];
  const wasm = wasmCandidates.find((p) => fs.existsSync(p));

  if (mode === "native") {
    return fs.existsSync(binary)
      ? { kind: "native", binary }
      : `no native server at ${binary}: build one with \`just lsp\` or set wirescript.lspPath`;
  }
  if (mode === "wasi") {
    return wasm
      ? { kind: "wasi", wasm }
      : `no WASI server at ${wasmCandidates.join(" or ")}: build one with \`just lsp-wasi\` or set wirescript.wasmPath`;
  }
  if (fs.existsSync(binary)) return { kind: "native", binary };
  if (wasm) return { kind: "wasi", wasm };
  return "no language server found: build one with `just lsp` or `just lsp-wasi`, or set wirescript.lspPath";
}

export function activate(context: ExtensionContext) {
  let extDir = context.extensionPath;
  try {
    extDir = fs.realpathSync(extDir);
  } catch {}

  registerDocs(context);

  // Diagnostics link to the docs through this handler (see docLinks.ts).
  const handlerBase = `${env.uriScheme}://${context.extension.id}`;
  context.subscriptions.push(
    window.registerUriHandler({
      handleUri(uri) {
        const page = new URLSearchParams(uri.query).get("page");
        if (uri.path === "/docs" && page) void commands.executeCommand(OPEN_DOCS_PAGE, page, uri.fragment || undefined);
      },
    }),
  );
  // A diagnostic code's docs link, pointed at the bundled page.
  const bundledCode = (code: Diagnostic["code"]): Diagnostic["code"] => {
    if (!code || typeof code !== "object") return code;
    const local = handlerUri(code.target.toString(true), handlerBase);
    return local ? { value: code.value, target: Uri.parse(local) } : code;
  };

  // On Windows the native server runs from a copy, so cargo can rebuild the
  // binary while it runs. Each copy is deleted once its server has stopped.
  function serverCopy(binary: string): string {
    if (process.platform !== "win32") return binary;
    const copyPath = path.join(os.tmpdir(), `wirescript-lsp-${Date.now()}.exe`);
    try {
      fs.copyFileSync(binary, copyPath);
      return copyPath;
    } catch {
      return binary;
    }
  }

  // One channel across restarts: the server's log, a WASI server's stderr,
  // and the compile log below.
  const output = window.createOutputChannel("Wirescript Language Server", { log: true });
  context.subscriptions.push(output);

  // Status bar item for compile progress
  const compileStatus = window.createStatusBarItem(1, 100);
  compileStatus.name = "Wirescript Compile";
  context.subscriptions.push(compileStatus);
  let compiling = false;

  // A WASI server sees the workspace folders mounted under /workspace or
  // /workspaces (see wasiPaths.ts), so URIs and paths
  // going to it, and paths coming back, are translated. Identity for native.
  let toServer = (uri: Uri) => uri.toString();
  let fromServerPath = (p: string) => Uri.file(p);
  // The WASI host mounts the workspace folders once, when the server starts.
  let mountsFolders = false;

  let watcher: fs.FSWatcher | undefined;
  context.subscriptions.push({ dispose: () => watcher?.close() });

  // Build-error diagnostics (from the on-demand Compile command). Kept in its own
  // collection so it never fights the language server's live typecheck diagnostics —
  // VS Code merges the two in the Problems panel. Cleared on each compile attempt
  // and on a server restart, since a rebuilt server may no longer report them.
  const buildDiagnostics = languages.createDiagnosticCollection("wirescript-build");
  context.subscriptions.push(buildDiagnostics);

  async function startClient() {
    const choice = chooseServer(extDir);
    if (typeof choice === "string") {
      output.appendLine(`[server] ${choice}`);
      window.showErrorMessage(`Wirescript: ${choice}`);
      return;
    }

    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ scheme: "file", language: "wirescript" }],
      synchronize: { fileEvents: workspace.createFileSystemWatcher("**/*.ws") },
      outputChannel: output,
      // The server links hovers and diagnostics to the published docs; open
      // the bundled pages instead.
      middleware: {
        provideHover: async (document, position, token, next) => {
          const hover = await next(document, position, token);
          if (!hover) return hover;
          hover.contents = hover.contents.map((part) => {
            if (!(part instanceof MarkdownString)) return part;
            const md = new MarkdownString(bundleDocLinks(part.value), part.supportThemeIcons);
            md.supportHtml = part.supportHtml;
            md.baseUri = part.baseUri;
            md.isTrusted = { enabledCommands: [OPEN_DOCS_PAGE] };
            return md;
          });
          return hover;
        },
        handleDiagnostics: (uri, diagnostics, next) => {
          for (const d of diagnostics) d.code = bundledCode(d.code);
          next(uri, diagnostics);
        },
      },
    };
    let serverOptions: ServerOptions;
    let source: string;
    if (choice.kind === "native") {
      const binPath = serverCopy(choice.binary);
      runningCopy = binPath === choice.binary ? undefined : binPath;
      const exe: Executable = { command: binPath };
      serverOptions = { run: exe, debug: exe };
      toServer = (uri) => uri.toString();
      fromServerPath = (p) => Uri.file(p);
      source = choice.binary;
      mountsFolders = false;
    } else {
      const mounts = wasiMounts(
        (workspace.workspaceFolders ?? []).map((f) => ({ uri: f.uri.toString(), name: f.name })),
      );
      clientOptions.uriConverters = {
        code2Protocol: (uri) => translateUri(uri.toString(), mounts, 0),
        protocol2Code: (value) => Uri.parse(translateUri(value, mounts, 1)),
      };
      toServer = (uri) => translateUri(uri.toString(), mounts, 0);
      fromServerPath = (p) => Uri.parse(translateUri(Uri.file(p).toString(), mounts, 1));
      serverOptions = () =>
        startWasiServer(Uri.file(choice.wasm), output, (p) => (wasiProcess = p), onWasiExit);
      source = choice.wasm;
      mountsFolders = true;
    }

    output.appendLine(
      `[server] starting the ${choice.kind === "wasi" ? "WASI" : "native"} server: ${source}`,
    );
    const started = new LanguageClient(
      "wirescript",
      "Wirescript Language Server",
      serverOptions,
      clientOptions,
    );
    // Compile progress. The compile command below owns the indicator's
    // lifecycle (shows it on start, clears it when the request resolves); a
    // progress notification only refreshes the step counter WHILE a compile is
    // actually in flight. `done` and any late/out-of-order notification are
    // ignored: they arrive fire-and-forget and used to strand the spinner when a
    // non-`done` was delivered last.
    started.onNotification("wirescript/compileProgress", (params: any) => {
      if (!compiling || params.done) return;
      // The server names the phase it is entering; showing it turns a stalled
      // "4/4" into "4/4 emit", which says where the time actually went. The
      // matching per-phase timings go to the LSP output channel server-side.
      const phase = params.label ? ` ${params.label}` : "";
      compileStatus.text = `$(sync~spin) Compiling: ${params.step}/${params.total}${phase}`;
      compileStatus.show();
    });
    client = started;
    watchForRebuild(source);
    try {
      await started.start();
    } catch (e: any) {
      output.appendLine(`[server] failed to start: ${e?.message ?? e}`);
      window.showErrorMessage(`Wirescript language server failed to start: ${e?.message ?? e}`);
    }
  }

  // A WASI server that dies is restarted, as the language client does for a
  // native one, unless it keeps dying.
  const crashTimes: number[] = [];
  function onWasiExit(ended: WasmProcess, reason: string) {
    // `stopClient` forgets the process before ending it.
    if (ended !== wasiProcess) return;
    output.appendLine(`[server] the WASI server ${reason}`);
    const now = Date.now();
    while (crashTimes.length > 0 && now - crashTimes[0] > CRASH_WINDOW_MS) crashTimes.shift();
    crashTimes.push(now);
    if (crashTimes.length > MAX_CRASHES) {
      window.showErrorMessage(
        `The Wirescript language server stopped ${crashTimes.length} times in a few minutes, so it was not restarted. ` +
          `The "Wirescript Language Server" output says why; run "Wirescript: Restart Language Server" to start it again.`,
      );
      return;
    }
    output.appendLine("[server] restarting");
    void restart();
  }

  // Restarts run one after another, so a burst of rebuilds or setting changes
  // never leaves two servers running.
  let restarting: Promise<void> = Promise.resolve();
  function restart(): Promise<void> {
    restarting = restarting
      .then(async () => {
        await stopClient();
        buildDiagnostics.clear();
        await startClient();
      })
      .catch((e: any) => output.appendLine(`[server] restart failed: ${e?.message ?? e}`));
    return restarting;
  }

  // Restart when the server being run is rebuilt (native binary or .wasm).
  function watchForRebuild(file: string) {
    watcher?.close();
    watcher = undefined;
    if (!fs.existsSync(file)) return;
    let debounce: NodeJS.Timeout | null = null;
    watcher = fs.watch(file, () => {
      if (debounce) clearTimeout(debounce);
      debounce = setTimeout(() => {
        debounce = null;
        output.appendLine("[server] restarting after a rebuild");
        void restart();
      }, 500);
    });
  }

  context.subscriptions.push(
    commands.registerCommand("wirescript.restartServer", () => restart()),
    workspace.onDidChangeConfiguration((e) => {
      if (["wirescript.server", "wirescript.lspPath", "wirescript.wasmPath"].some((k) => e.affectsConfiguration(k))) {
        void restart();
      }
    }),
    workspace.onDidChangeWorkspaceFolders(() => {
      if (mountsFolders) void restart();
    }),
  );

  void restart();

  // Alt-Shift-O: organize imports — sort alphabetically, remove unused
  context.subscriptions.push(
    languages.registerCodeActionsProvider(
      { language: "wirescript" },
      {
        provideCodeActions(document: TextDocument): CodeAction[] {
          const text = document.getText();
          const lines = text.split(/\r?\n/);
          const importLines: {
            idx: number; // first source line of this import
            idxEnd: number; // last source line (> idx for multi-line named imports)
            line: string;
            path: string;
            kind: "named" | "namespace" | "bare";
            names: string[]; // named imports only
            alias: string; // namespace imports only (`import * as <alias>`)
          }[] = [];
          const nonImportIdents = new Set<string>();

          // Parse import lines and collect all idents from non-import lines.
          // Track block-comment state across lines so identifiers inside /* ... */
          // are not counted as usage.
          let inBlockComment = false;
          for (let i = 0; i < lines.length; i++) {
            // Recognize all three import forms. Previously only the braced/named
            // form matched; namespace (`import * as ns from "x"`) and bare
            // (`import "x"`) imports fell through to the code scanner and — worse
            // — were deleted whenever they sat between two named imports, since
            // the whole span gets replaced by the rebuilt named imports.
            const named = lines[i].match(
              /^import\s*\{([^}]+)\}\s*from\s*"([^"]+)"/,
            );
            const namespace = lines[i].match(
              /^import\s*\*\s*as\s+(\w+)\s+from\s*"([^"]+)"/,
            );
            const bare = lines[i].match(/^import\s+"([^"]+)"\s*$/);
            if (named && !inBlockComment) {
              const names = named[1]
                .split(",")
                .map((s) => s.trim())
                .filter(Boolean);
              importLines.push({
                idx: i,
                idxEnd: i,
                line: lines[i],
                path: named[2],
                kind: "named",
                names,
                alias: "",
              });
            } else if (namespace && !inBlockComment) {
              importLines.push({
                idx: i,
                idxEnd: i,
                line: lines[i],
                path: namespace[2],
                kind: "namespace",
                names: [],
                alias: namespace[1],
              });
            } else if (bare && !inBlockComment) {
              importLines.push({
                idx: i,
                idxEnd: i,
                line: lines[i],
                path: bare[1],
                kind: "bare",
                names: [],
                alias: "",
              });
            } else if (!inBlockComment && /^import\s*\{/.test(lines[i])) {
              // Multi-line named import: `import {` opens here but the closing
              // `} from "path"` is on a later line. Accumulate until we reach it.
              // Without this the continuation lines read as code and the whole
              // import is destroyed by the span replacement below.
              let buf = lines[i];
              let j = i;
              while (
                j + 1 < lines.length &&
                !/\}\s*from\s*"[^"]+"/.test(buf)
              ) {
                j++;
                buf += "\n" + lines[j];
              }
              const multi = buf.match(
                /^import\s*\{([\s\S]*?)\}\s*from\s*"([^"]+)"/,
              );
              if (multi) {
                const names = multi[1]
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean);
                importLines.push({
                  idx: i,
                  idxEnd: j,
                  line: lines[i],
                  path: multi[2],
                  kind: "named",
                  names,
                  alias: "",
                });
                i = j; // skip the consumed continuation lines
              }
            } else {
              // Strip comments and string literals before scanning for identifiers.
              // Process character by character to handle //, /* */, and "" '' strings.
              let code = "";
              let j = 0;
              const raw = lines[i];
              while (j < raw.length) {
                if (inBlockComment) {
                  // Look for end of block comment
                  if (raw[j] === "*" && raw[j + 1] === "/") {
                    inBlockComment = false;
                    j += 2;
                  } else {
                    j++;
                  }
                } else if (raw[j] === "/" && raw[j + 1] === "*") {
                  // Start of block comment
                  inBlockComment = true;
                  j += 2;
                } else if (raw[j] === "/" && raw[j + 1] === "/") {
                  // Line comment — skip the rest of the line
                  break;
                } else if (raw[j] === '"' || raw[j] === "'") {
                  // String literal — skip its literal text, but DESCEND into any
                  // ${ ... } interpolation. Identifiers used only inside interpolated
                  // strings (e.g. "${teamName(t)}") are real usages; skipping them
                  // makes an import look unused and silently prunes it, breaking the
                  // build. The interpolation body is code, so feed it to the scanner.
                  const quote = raw[j];
                  j++;
                  while (j < raw.length && raw[j] !== quote) {
                    if (raw[j] === "\\") {
                      j += 2; // skip escape + escaped char
                    } else if (raw[j] === "$" && raw[j + 1] === "{") {
                      // Delimit each interpolation so back-to-back ones ("${a}${b}",
                      // or "${a}">${b}") don't merge into a single bogus identifier.
                      code += " ";
                      j += 2;
                      let depth = 1;
                      while (j < raw.length && depth > 0) {
                        if (raw[j] === "{") {
                          depth++;
                        } else if (raw[j] === "}") {
                          depth--;
                          if (depth === 0) {
                            j++;
                            break;
                          }
                        }
                        code += raw[j];
                        j++;
                      }
                    } else {
                      j++;
                    }
                  }
                  j++; // skip closing quote
                } else {
                  code += raw[j];
                  j++;
                }
              }
              // Collect all word-like identifiers from the code portion only
              const idents = code.match(/\b[a-zA-Z_]\w*\b/g);
              if (idents) idents.forEach((id) => nonImportIdents.add(id));
            }
          }

          if (importLines.length === 0) return [];

          // Prune unused NAMED imports; always preserve namespace + bare imports
          // (their usage can't be determined reliably from identifiers alone, and
          // wrongly removing one silently breaks the build). Re-render each, then
          // sort by path; same-path ties order by rendered line, so `import *`
          // sorts before `import {` (as the existing convention has it).
          const cleaned = importLines
            .map((imp) => {
              if (imp.kind === "named") {
                const used = imp.names.filter((n) => nonImportIdents.has(n));
                if (used.length === 0) return null;
                used.sort();
                return {
                  ...imp,
                  names: used,
                  line: `import { ${used.join(", ")} } from "${imp.path}"`,
                };
              }
              if (imp.kind === "namespace") {
                return {
                  ...imp,
                  line: `import * as ${imp.alias} from "${imp.path}"`,
                };
              }
              return { ...imp, line: `import "${imp.path}"` };
            })
            .filter(Boolean) as typeof importLines;
          const kindRank = (k: string) =>
            k === "namespace" ? 0 : k === "named" ? 1 : 2;
          cleaned.sort(
            (a, b) =>
              a.path.localeCompare(b.path) ||
              kindRank(a.kind) - kindRank(b.kind) ||
              a.line.localeCompare(b.line),
          );

          // Build the replacement. The span must cover every source line the
          // imports occupy — including multi-line named imports' continuations
          // (idxEnd > idx) — or trailing continuation lines would survive as
          // orphaned text after the rebuilt block.
          const firstIdx = importLines[0].idx;
          const lastIdx = Math.max(...importLines.map((imp) => imp.idxEnd));
          const newImports = cleaned.map((c) => c.line).join("\n");
          const range = new Range(
            new Position(firstIdx, 0),
            new Position(lastIdx, lines[lastIdx].length),
          );

          const action = new CodeAction(
            "Organize Imports",
            CodeActionKind.SourceOrganizeImports,
          );
          action.edit = new WorkspaceEdit();
          action.edit.replace(document.uri, range, newImports);
          return [action];
        },
      },
      { providedCodeActionKinds: [CodeActionKind.SourceOrganizeImports] },
    ),
  );

  // A change to ANY .ws file makes the last compile's output stale as a whole
  // (its errors can point into imported files), so drop the entire collection —
  // the live LSP diagnostics keep reporting truth, and the next explicit
  // compile repopulates it. Without this, fixed errors lingered as squiggles
  // next to the fresh live diagnostics until the next compile.
  const clearBuildDiagnostics = (doc: TextDocument) => {
    if (doc.languageId === "wirescript") buildDiagnostics.clear();
  };
  context.subscriptions.push(
    workspace.onDidChangeTextDocument((e) => {
      if (e.contentChanges.length > 0) clearBuildDiagnostics(e.document);
    }),
    workspace.onDidSaveTextDocument(clearBuildDiagnostics),
  );

  // Client-side half of the compile log. The server logs the compile itself
  // (phases, timings, diagnostics) into this same channel via window/logMessage;
  // these lines cover the steps only the extension can see.
  const logCompile = (msg: string) => output.appendLine(`[compile] ${msg}`);

  // Compile and copy .brz to clipboard as file drop (for Brickadia paste)
  context.subscriptions.push(
    commands.registerCommand("wirescript.compileAndCopy", async () => {
      const doc = window.activeTextEditor?.document;
      if (!doc || doc.languageId !== "wirescript") {
        window.showWarningMessage("No active Wirescript file.");
        return;
      }
      const running = client;
      if (!running) {
        window.showErrorMessage("Wirescript: the language server is not running.");
        return;
      }
      await doc.save();
      const baseName = path.basename(doc.uri.fsPath, ".ws");
      const outPath = path.join(os.tmpdir(), `${baseName}.brz`);

      compiling = true;
      compileStatus.text = "$(sync~spin) Compiling...";
      compileStatus.show();

      // Fresh compile — drop any stale build errors before we try again.
      buildDiagnostics.clear();

      // No output path: the server returns the save (base64) and it is written
      // here, since a WASI server cannot see the temp folder.
      let result: any;
      try {
        result = await running.sendRequest("workspace/executeCommand", {
          command: "wirescript.compile",
          arguments: [toServer(doc.uri)],
        });
      } catch (err: any) {
        compileStatus.text = "$(error) Compile failed";
        setTimeout(() => compileStatus.hide(), 5000);
        logCompile(`request failed: ${err.message || err}`);
        window.showErrorMessage(
          `Wirescript compile failed: ${err.message || err}`,
        );
        return;
      } finally {
        // The request has resolved — the compile is truly done, so stop
        // honoring any further (late) progress notifications for the spinner.
        compiling = false;
      }

      // Build errors come back located — render them in the Problems panel + as
      // squiggles (grouped by file, since a cycle can span imported modules).
      if (result && result.ok === false) {
        const byFile = new Map<string, Diagnostic[]>();
        for (const d of result.diagnostics ?? []) {
          const uri = d.file ? fromServerPath(d.file) : doc.uri;
          const key = uri.toString();
          const range = new Range(
            d.startLine ?? 0,
            d.startChar ?? 0,
            d.endLine ?? 0,
            d.endChar ?? 0,
          );
          const severity =
            d.severity === "warning"
              ? DiagnosticSeverity.Warning
              : d.severity === "info"
                ? DiagnosticSeverity.Information
                : DiagnosticSeverity.Error;
          const diag = new Diagnostic(range, d.message, severity);
          if (d.code) diag.code = d.docUrl ? bundledCode({ value: d.code, target: Uri.parse(d.docUrl) }) : d.code;
          diag.source = "wirescript build";
          const list = byFile.get(key) ?? [];
          list.push(diag);
          byFile.set(key, list);
        }
        for (const [key, diags] of byFile) {
          buildDiagnostics.set(Uri.parse(key), diags);
        }
        const n = (result.diagnostics ?? []).length;
        compileStatus.text = "$(error) Compile failed";
        setTimeout(() => compileStatus.hide(), 5000);
        window.showErrorMessage(
          `Wirescript compile failed: ${n} problem${n === 1 ? "" : "s"} — see the Problems panel.`,
        );
        return;
      }

      // The build itself is done, so retire the persistent indicator BEFORE the
      // clipboard hand-off; success is reported by the transient status-bar
      // message below. Clearing it here means a slow or wedged clipboard helper
      // can never again strand the spinner on the last compile step.
      compileStatus.hide();

      try {
        fs.writeFileSync(outPath, Buffer.from(result?.brz ?? "", "base64"));
      } catch (err: any) {
        logCompile(`writing ${outPath} failed: ${err.message || err}`);
        window.showErrorMessage(`Wirescript: could not write ${outPath}: ${err.message || err}`);
        return;
      }

      if (copyFileToClipboard(outPath)) {
        logCompile(`copied ${baseName}.brz to the clipboard as a file drop`);
        window.setStatusBarMessage(
          `$(check) Compiled → ${baseName}.brz (copied to clipboard)`,
          5000,
        );
      } else {
        env.clipboard.writeText(outPath);
        // Only place this is explained: the status bar just says "path copied",
        // which looks like success until you try to paste into Brickadia.
        logCompile(
          process.platform === "linux"
            ? `no clipboard helper worked, copied the path as text instead — install wl-clipboard (Wayland) or xclip (X11) for a file drop`
            : `no clipboard helper worked, copied the path as text instead`,
        );
        window.setStatusBarMessage(
          `$(check) Compiled → ${outPath} (path copied)`,
          5000,
        );
      }
    }),
  );

  // For the built-in markdown preview (`markdown.markdownItPlugins`):
  // ```wirescript blocks colored with this extension's grammar, and the
  // bundled docs' book links pointed at their markdown pages.
  const docsDir = path.join(extDir, "docs");
  return loadGrammar(extDir).then(
    (grammar) => ({ extendMarkdownIt: wirescriptMarkdown(grammar, docsDir) }),
    (e: any) => {
      output.appendLine(`[docs] code blocks stay plain: the grammar did not load (${e?.message ?? e})`);
      return { extendMarkdownIt: wirescriptMarkdown(null, docsDir) };
    },
  );
}

export function deactivate(): Thenable<void> | undefined {
  return stopClient();
}
