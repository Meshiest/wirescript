// "Wirescript: Play": the active `.ws` file live in a webview, its displays
// drawn and its seats clickable. The simulation runs in a worker thread of
// the extension host (dist/runner/worker.js, the shared loop over the Node
// build of the wasm crate); the webview only draws and sends script lines.

import * as path from "node:path";
import { Worker } from "node:worker_threads";
import * as vscode from "vscode";
import { moduleName, runnerHtml, siblingFiles, viewMessage } from "./runnerPage";

/** The most sibling files, and the largest one, a program's imports read. */
const MAX_IMPORT_FILES = 256;
const MAX_IMPORT_BYTES = 1 << 20;

/** The worker's memory: the simulation of one program. */
const WORKER_LIMITS = { maxOldGenerationSizeMb: 1024, stackSizeMb: 8 };

/** Whether two file paths name one file, ignoring case where the OS does. */
function samePath(a: string, b: string): boolean {
  return process.platform === "win32" ? a.toLowerCase() === b.toLowerCase() : a === b;
}

/** The `.ws` files beside `doc`, for its imports. */
async function importFiles(doc: vscode.TextDocument): Promise<string> {
  const dir = vscode.Uri.joinPath(doc.uri, "..");
  const active = path.basename(doc.uri.fsPath);
  const entries: { name: string; text: string }[] = [];
  let listing: [string, vscode.FileType][] = [];
  try {
    listing = await vscode.workspace.fs.readDirectory(dir);
  } catch {
    return "{}";
  }
  for (const [name, type] of listing) {
    if (entries.length >= MAX_IMPORT_FILES) break;
    if (type !== vscode.FileType.File || !name.endsWith(".ws") || name === active) continue;
    const uri = vscode.Uri.joinPath(dir, name);
    const open = vscode.workspace.textDocuments.find((d) => samePath(d.uri.fsPath, uri.fsPath));
    let text: string;
    if (open) {
      text = open.getText();
    } else {
      const stat = await vscode.workspace.fs.stat(uri);
      if (stat.size > MAX_IMPORT_BYTES) continue;
      text = new TextDecoder().decode(await vscode.workspace.fs.readFile(uri));
    }
    entries.push({ name, text });
  }
  return siblingFiles(entries, active);
}

/** Open the runner for the active `.ws` editor. */
export async function openRunner(context: vscode.ExtensionContext): Promise<void> {
  const doc = vscode.window.activeTextEditor?.document;
  if (!doc || doc.languageId !== "wirescript") {
    void vscode.window.showWarningMessage("Wirescript: open a .ws file to play it.");
    return;
  }
  const root = vscode.Uri.joinPath(context.extensionUri, "dist", "runner");
  const fileName = path.basename(doc.uri.fsPath);
  const source = doc.getText();
  // A file not on disk (an untitled buffer) has no siblings to import.
  const files = doc.uri.scheme === "file" ? await importFiles(doc) : "{}";
  const panel = vscode.window.createWebviewPanel(
    "wirescript.runner",
    `Play: ${fileName}`,
    vscode.ViewColumn.Beside,
    // Kept while hidden, so a run carries on behind another tab rather than
    // starting over when the page is shown again.
    { enableScripts: true, localResourceRoots: [root], retainContextWhenHidden: true },
  );
  const asset = (name: string) => panel.webview.asWebviewUri(vscode.Uri.joinPath(root, name)).toString();
  panel.webview.html = runnerHtml(
    {
      cspSource: panel.webview.cspSource,
      glue: asset("webview.js"),
      css: asset("runner.css"),
      pageCss: asset("page.css"),
    },
    `Play: ${fileName}`,
  );

  let disposed = false;
  const toPage = (msg: unknown) => {
    if (!disposed) void panel.webview.postMessage(msg);
  };
  const worker = new Worker(path.join(root.fsPath, "worker.js"), { resourceLimits: WORKER_LIMITS });
  worker.on("message", toPage);
  worker.on("error", (e: Error) => toPage({ type: "error", error: `the runner stopped: ${e.message}` }));
  const listening = panel.webview.onDidReceiveMessage((raw) => {
    const msg = viewMessage(raw);
    if (msg?.type === "ready") {
      worker.postMessage({ type: "start", source, moduleName: moduleName(fileName), filesJson: files, prefabsJson: "{}" });
    } else if (msg?.type === "line") {
      worker.postMessage(msg);
    }
  });
  panel.onDidDispose(() => {
    disposed = true;
    listening.dispose();
    worker.removeAllListeners();
    worker.on("error", () => {});
    void worker.terminate();
  });
}
