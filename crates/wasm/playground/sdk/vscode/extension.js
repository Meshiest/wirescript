// WASM-powered Wirescript VS Code extension (no native binary needed)
const vscode = require("vscode");
const path = require("path");
const fs = require("fs");

let wasm = null;

async function initWasm(extDir) {
  if (wasm) return wasm;
  const wasmJsPath = path.join(extDir, "pkg", "wasm.js");
  const wasmBinPath = path.join(extDir, "pkg", "wasm_bg.wasm");
  if (!fs.existsSync(wasmJsPath) || !fs.existsSync(wasmBinPath)) return null;
  const mod = require(wasmJsPath);
  const wasmBytes = fs.readFileSync(wasmBinPath);
  await mod.default({ module_or_path: wasmBytes });
  wasm = mod;
  return wasm;
}

function buildFileMap(excludeUri) {
  const map = {};
  const baseDir = excludeUri ? path.dirname(excludeUri.fsPath) : null;
  try {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders) return "{}";
    for (const folder of folders) {
      const root = folder.uri.fsPath;
      const walk = (dir) => {
        try {
          for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
            if (entry.isDirectory() && !entry.name.startsWith(".") && entry.name !== "node_modules") {
              walk(path.join(dir, entry.name));
            } else if (entry.isFile() && entry.name.endsWith(".ws")) {
              const full = path.join(dir, entry.name);
              if (excludeUri && full === excludeUri.fsPath) continue;
              const rel = baseDir ? path.relative(baseDir, full) : path.relative(root, full);
              map[rel] = fs.readFileSync(full, "utf-8");
            }
          }
        } catch {}
      };
      walk(root);
    }
  } catch {}
  return JSON.stringify(map);
}

async function activate(context) {
  const extDir = context.extensionPath;
  const w = await initWasm(extDir).catch(() => null);
  if (!w) {
    vscode.window.showWarningMessage("Wirescript: WASM not found in extension pkg/. Language features disabled.");
    return;
  }

  const LANG = "wirescript";
  let timer = null;
  const diags = vscode.languages.createDiagnosticCollection(LANG);
  context.subscriptions.push(diags);

  function getFilesJson(doc) {
    return buildFileMap(doc.uri);
  }

  function update(doc) {
    if (doc.languageId !== LANG) return;
    try {
      const items = JSON.parse(w.wirescript_diagnostics(doc.getText(), getFilesJson(doc)));
      diags.set(doc.uri, items.map(d => new vscode.Diagnostic(
        new vscode.Range(d.startLine, d.startCol, d.endLine || d.startLine, d.endCol || d.startCol + 1),
        d.message,
        d.severity === "error" ? 0 : d.severity === "warning" ? 1 : 2
      )));
    } catch { diags.set(doc.uri, []); }
  }

  context.subscriptions.push(
    vscode.workspace.onDidChangeTextDocument(e => { clearTimeout(timer); timer = setTimeout(() => update(e.document), 500); }),
    vscode.workspace.onDidOpenTextDocument(update),
  );
  vscode.workspace.textDocuments.forEach(update);

  context.subscriptions.push(vscode.languages.registerCompletionItemProvider(LANG, {
    provideCompletionItems(doc, pos) {
      try {
        const fj = getFilesJson(doc);
        const items = JSON.parse(w.wirescript_completions(doc.getText(), pos.line, pos.character, fj));
        const results = items.map(i => {
          const k = { function: 1, keyword: 13, type: 6, class: 6, field: 4, event: 22, method: 1, var: 5, buffer: 5, let: 20, mod: 1, chip: 1, param: 5 }[i.kind] || 0;
          const ci = new vscode.CompletionItem(i.label, k);
          ci.detail = i.detail || undefined;
          if ((i.kind === "function" || i.kind === "method" || i.kind === "mod" || i.kind === "chip") && !i.insertText)
            ci.insertText = new vscode.SnippetString(i.label + "($1)");
          else if (i.insertText?.includes("$")) ci.insertText = new vscode.SnippetString(i.insertText);
          else if (i.insertText) ci.insertText = i.insertText;
          return ci;
        });

        // Auto-import: suggest symbols from other workspace files
        if (results.length === 0 && fj !== "{}") {
          try {
            const wsSyms = JSON.parse(w.wirescript_workspace_symbols(fj));
            const word = doc.getText(doc.getWordRangeAtPosition(pos));
            const matches = wsSyms.filter(s => s.name.toLowerCase().startsWith(word.toLowerCase()));
            for (const s of matches) {
              const ci = new vscode.CompletionItem(s.name, s.kind === "mod" || s.kind === "chip" ? 1 : 5);
              ci.detail = `(auto-import from ${s.file.replace(/\.ws$/, "")})`;
              const importPath = s.file.replace(/\.ws$/, "");
              ci.additionalTextEdits = [
                vscode.TextEdit.insert(new vscode.Position(0, 0), `import { ${s.name} } from "${importPath}"\n`)
              ];
              if (s.kind === "mod" || s.kind === "chip" || s.kind === "fn")
                ci.insertText = new vscode.SnippetString(s.name + "($1)");
              results.push(ci);
            }
          } catch {}
        }

        return results;
      } catch { return []; }
    }
  }, ".", "("));

  context.subscriptions.push(vscode.languages.registerHoverProvider(LANG, {
    provideHover(doc, pos) {
      try {
        const r = w.wirescript_hover(doc.getText(), pos.line, pos.character, getFilesJson(doc));
        if (!r) return null;
        let v = r; try { v = JSON.parse(r).value; } catch {}
        return new vscode.Hover(new vscode.MarkdownString(v));
      } catch { return null; }
    }
  }));

  context.subscriptions.push(vscode.languages.registerDefinitionProvider(LANG, {
    provideDefinition(doc, pos) {
      try {
        const r = w.wirescript_definition(doc.getText(), pos.line, pos.character, getFilesJson(doc));
        if (!r) return null;
        const l = JSON.parse(r);
        let uri = doc.uri;
        if (l.file) {
          const resolved = path.resolve(path.dirname(doc.uri.fsPath), l.file);
          uri = vscode.Uri.file(resolved);
        }
        return new vscode.Location(uri, new vscode.Range(l.startLine, l.startCol, l.endLine, l.endCol));
      } catch { return null; }
    }
  }));

  context.subscriptions.push(vscode.languages.registerReferenceProvider(LANG, {
    provideReferences(doc, pos) {
      try {
        const refs = JSON.parse(w.wirescript_references(doc.getText(), pos.line, pos.character, getFilesJson(doc)));
        return refs.map(r => new vscode.Location(doc.uri, new vscode.Range(r.startLine, r.startCol, r.endLine, r.endCol)));
      } catch { return []; }
    }
  }));

  context.subscriptions.push(vscode.languages.registerDocumentFormattingEditProvider(LANG, {
    provideDocumentFormattingEdits(doc, opts) {
      try {
        const f = w.wirescript_format(doc.getText(), opts.tabSize, !opts.insertSpaces);
        if (f === doc.getText()) return [];
        return [vscode.TextEdit.replace(new vscode.Range(0, 0, doc.lineCount, 0), f)];
      } catch { return []; }
    }
  }));

  context.subscriptions.push(vscode.languages.registerRenameProvider(LANG, {
    provideRenameEdits(doc, pos, newName) {
      try {
        const refs = JSON.parse(w.wirescript_references(doc.getText(), pos.line, pos.character));
        const edit = new vscode.WorkspaceEdit();
        refs.forEach(r => edit.replace(doc.uri, new vscode.Range(r.startLine, r.startCol, r.endLine, r.endCol), newName));
        return edit;
      } catch { return undefined; }
    }
  }));

  // Compile and copy .brz to clipboard as file drop (for Brickadia paste)
  context.subscriptions.push(vscode.commands.registerCommand("wirescript.compileAndCopy", async () => {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || doc.languageId !== LANG) {
      vscode.window.showWarningMessage("No active Wirescript file.");
      return;
    }
    await doc.save();
    try {
      const source = doc.getText();
      const baseName = path.basename(doc.uri.fsPath, ".ws");
      const os = require("os");
      const outPath = path.join(os.tmpdir(), `${baseName}.brz`);
      const filesJson = getFilesJson(doc);
      const bytes = w.wirescript_compile(source, baseName, filesJson);
      fs.writeFileSync(outPath, Buffer.from(bytes));
      const { execSync } = require("child_process");
      try {
        if (process.platform === "win32") {
          execSync(`powershell -command "Set-Clipboard -Path '${outPath.replace(/'/g, "''")}'"`);
        } else if (process.platform === "darwin") {
          execSync(`osascript -e 'set the clipboard to POSIX file "${outPath}"'`);
        } else {
          execSync(`xclip -selection clipboard -t text/uri-list -i <<< "file://${outPath}"`);
        }
        vscode.window.showInformationMessage(`Compiled → ${baseName}.brz (copied to clipboard)`);
      } catch {
        await vscode.env.clipboard.writeText(outPath);
        vscode.window.showInformationMessage(`Compiled → ${outPath} (path copied)`);
      }
    } catch (e) {
      vscode.window.showErrorMessage(`Wirescript compile failed: ${e}`);
    }
  }));
}

function deactivate() {}

module.exports = { activate, deactivate };
