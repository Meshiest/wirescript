import * as path from "path";
import * as os from "os";
import {
  workspace,
  ExtensionContext,
  languages,
  commands,
  window,
  env,
  Uri,
  DocumentFormattingEditProvider,
  CodeActionProvider,
  CodeActionKind,
  CodeAction,
  TextDocument,
  FormattingOptions,
  CancellationToken,
  TextEdit,
  Range,
  Position,
  WorkspaceEdit,
  Diagnostic,
  DiagnosticSeverity,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Executable,
} from "vscode-languageclient/node";

let client: LanguageClient;

class PrettierFormatter implements DocumentFormattingEditProvider {
  private extDir: string;

  constructor(extDir: string) {
    this.extDir = extDir;
  }

  async provideDocumentFormattingEdits(
    document: TextDocument,
    options: FormattingOptions,
    _token: CancellationToken,
  ): Promise<TextEdit[]> {
    try {
      const prettierPath = path.join(this.extDir, "node_modules", "prettier");
      const pluginPath = path.join(
        this.extDir,
        "prettier-plugin-wirescript.js",
      );
      const prettier = require(prettierPath);
      const source = document.getText();
      const formatted = await prettier.format(source, {
        parser: "wirescript",
        plugins: [pluginPath],
        tabWidth: options.tabSize,
        useTabs: !options.insertSpaces,
      });
      if (formatted === source) return [];
      const fullRange = new Range(
        new Position(0, 0),
        document.lineAt(document.lineCount - 1).range.end,
      );
      return [TextEdit.replace(fullRange, formatted)];
    } catch (e: any) {
      const { window } = require("vscode");
      window.showErrorMessage(`Wirescript format error: ${e.message}`);
      console.error("wirescript prettier format error:", e.stack || e.message);
      return [];
    }
  }
}

export function activate(context: ExtensionContext) {
  const config = workspace.getConfiguration("wirescript");
  let serverPath = config.get<string>("lspPath", "");

  const fs = require("fs");
  let extDir = context.extensionPath;
  try {
    extDir = fs.realpathSync(extDir);
  } catch {}
  const repoRoot = path.resolve(extDir, "..", "..");

  if (!serverPath) {
    const ext = process.platform === "win32" ? ".exe" : "";
    serverPath = path.join(
      repoRoot,
      "target",
      "release",
      `wirescript-lsp${ext}`,
    );
  }

  // On Windows, copy the binary so cargo can rebuild while the LSP is running
  if (process.platform === "win32" && fs.existsSync(serverPath)) {
    const copyPath = path.join(os.tmpdir(), `wirescript-lsp-${Date.now()}.exe`);
    try {
      fs.copyFileSync(serverPath, copyPath);
      serverPath = copyPath;
    } catch {}
  }

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "wirescript" }],
    synchronize: { fileEvents: workspace.createFileSystemWatcher("**/*.ws") },
    // The extension registers its prettier formatter below; tell the server
    // not to advertise formatting so the picker shows a single entry.
    initializationOptions: { provideFormatting: false },
  };

  // Track the source binary path (before copy) so we can watch for rebuilds
  const sourceBinaryPath =
    config.get<string>("lspPath", "") ||
    path.join(
      repoRoot,
      "target",
      "release",
      `wirescript-lsp${process.platform === "win32" ? ".exe" : ""}`,
    );

  function startClient(binPath: string) {
    const exe: Executable = { command: binPath };
    const opts: ServerOptions = { run: exe, debug: exe };
    client = new LanguageClient(
      "wirescript",
      "Wirescript Language Server",
      opts,
      clientOptions,
    );
    client.start();
  }

  startClient(serverPath);

  // Prettier-based formatter — the sole formatting provider (see
  // initializationOptions above).
  context.subscriptions.push(
    languages.registerDocumentFormattingEditProvider(
      { language: "wirescript" },
      new PrettierFormatter(extDir),
    ),
  );

  // Watch the source binary for rebuilds and auto-restart the LSP
  if (fs.existsSync(sourceBinaryPath)) {
    let debounce: NodeJS.Timeout | null = null;
    fs.watch(sourceBinaryPath, () => {
      if (debounce) clearTimeout(debounce);
      debounce = setTimeout(async () => {
        debounce = null;
        try {
          let newPath = sourceBinaryPath;
          if (process.platform === "win32") {
            const copyPath = path.join(
              os.tmpdir(),
              `wirescript-lsp-${Date.now()}.exe`,
            );
            fs.copyFileSync(sourceBinaryPath, copyPath);
            newPath = copyPath;
          }
          if (client) await client.stop();
          startClient(newPath);
          console.log("wirescript-lsp: restarted after binary change");
        } catch (e: any) {
          console.error("wirescript-lsp: restart failed:", e.message);
        }
      }, 500);
    });
  }

  // Alt-Shift-O: organize imports — sort alphabetically, remove unused
  context.subscriptions.push(
    languages.registerCodeActionsProvider(
      { language: "wirescript" },
      {
        provideCodeActions(document: TextDocument): CodeAction[] {
          const text = document.getText();
          const lines = text.split(/\r?\n/);
          const importLines: {
            idx: number;
            line: string;
            path: string;
            names: string[];
          }[] = [];
          const nonImportIdents = new Set<string>();

          // Parse import lines and collect all idents from non-import lines.
          // Track block-comment state across lines so identifiers inside /* ... */
          // are not counted as usage.
          let inBlockComment = false;
          for (let i = 0; i < lines.length; i++) {
            const m = lines[i].match(
              /^import\s*\{([^}]+)\}\s*from\s*"([^"]+)"/,
            );
            if (m && !inBlockComment) {
              const names = m[1]
                .split(",")
                .map((s) => s.trim())
                .filter(Boolean);
              importLines.push({ idx: i, line: lines[i], path: m[2], names });
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

          // Remove unused imports, sort remaining names, sort import lines by path
          const cleaned = importLines
            .map((imp) => {
              const used = imp.names.filter((n) => nonImportIdents.has(n));
              if (used.length === 0) return null;
              used.sort();
              return {
                ...imp,
                names: used,
                line: `import { ${used.join(", ")} } from "${imp.path}"`,
              };
            })
            .filter(Boolean) as typeof importLines;
          cleaned.sort((a, b) => a.path.localeCompare(b.path));

          // Build the replacement
          const firstIdx = importLines[0].idx;
          const lastIdx = importLines[importLines.length - 1].idx;
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

  // Status bar item for compile progress
  const compileStatus = window.createStatusBarItem(1, 100);
  compileStatus.name = "Wirescript Compile";
  context.subscriptions.push(compileStatus);

  // Build-error diagnostics (from the on-demand Compile command). Kept in its own
  // collection so it never fights the language server's live typecheck diagnostics —
  // VS Code merges the two in the Problems panel. Cleared on each compile attempt.
  const buildDiagnostics = languages.createDiagnosticCollection("wirescript-build");
  context.subscriptions.push(buildDiagnostics);

  // Listen for compile progress from LSP
  client.onNotification("wirescript/compileProgress", (params: any) => {
    if (params.done) {
      compileStatus.text = `$(check) Compiled`;
      setTimeout(() => compileStatus.hide(), 5000);
    } else {
      compileStatus.text = `$(sync~spin) Compiling: ${params.step}/${params.total}`;
      compileStatus.show();
    }
  });

  // Compile and copy .brz to clipboard as file drop (for Brickadia paste)
  context.subscriptions.push(
    commands.registerCommand("wirescript.compileAndCopy", async () => {
      const doc = window.activeTextEditor?.document;
      if (!doc || doc.languageId !== "wirescript") {
        window.showWarningMessage("No active Wirescript file.");
        return;
      }
      await doc.save();
      const baseName = path.basename(doc.uri.fsPath, ".ws");
      const outPath = path.join(os.tmpdir(), `${baseName}.brz`);

      compileStatus.text = "$(sync~spin) Compiling...";
      compileStatus.show();

      // Fresh compile — drop any stale build errors before we try again.
      buildDiagnostics.clear();

      let result: any;
      try {
        result = await client.sendRequest("workspace/executeCommand", {
          command: "wirescript.compile",
          arguments: [doc.uri.toString(), outPath],
        });
      } catch (err: any) {
        compileStatus.text = "$(error) Compile failed";
        setTimeout(() => compileStatus.hide(), 5000);
        window.showErrorMessage(
          `Wirescript compile failed: ${err.message || err}`,
        );
        return;
      }

      // Build errors come back located — render them in the Problems panel + as
      // squiggles (grouped by file, since a cycle can span imported modules).
      if (result && result.ok === false) {
        const byFile = new Map<string, Diagnostic[]>();
        for (const d of result.diagnostics ?? []) {
          const uri = d.file ? Uri.file(d.file) : doc.uri;
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
          if (d.code) diag.code = d.code;
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

      const { execSync } = require("child_process");
      try {
        if (process.platform === "win32") {
          execSync(
            `powershell -command "Set-Clipboard -Path '${outPath.replace(/'/g, "''")}'"`,
          );
        } else if (process.platform === "darwin") {
          execSync(
            `osascript -e 'set the clipboard to POSIX file "${outPath}"'`,
          );
        } else {
          execSync(
            `xclip -selection clipboard -t text/uri-list -i <<< "file://${outPath}"`,
          );
        }
        window.setStatusBarMessage(
          `$(check) Compiled → ${baseName}.brz (copied to clipboard)`,
          5000,
        );
      } catch {
        env.clipboard.writeText(outPath);
        window.setStatusBarMessage(
          `$(check) Compiled → ${outPath} (path copied)`,
          5000,
        );
      }
    }),
  );
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
