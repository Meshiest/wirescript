// The parts of the runner webview that need no `vscode` API: its page, the
// check on what the page may send, and the files a program's imports read.

/** Where the page's files are, as webview URIs. */
export interface RunnerAssets {
  /** `webview.cspSource`. */
  cspSource: string;
  glue: string;
  /** The shared runner stylesheet. */
  css: string;
  /** This page's own layout. */
  pageCss: string;
}

const escapeHtml = (s: string): string =>
  s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/**
 * The webview's page: the shared runner view and its stylesheet, under a
 * policy that loads scripts and styles from the extension's own files only.
 */
export function runnerHtml(assets: RunnerAssets, title: string): string {
  const csp = [
    "default-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
    `script-src ${assets.cspSource}`,
    `style-src ${assets.cspSource}`,
  ].join("; ");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title)}</title>
<link rel="stylesheet" href="${assets.css}">
<link rel="stylesheet" href="${assets.pageCss}">
</head>
<body>
<div id="runner"></div>
<script type="module" src="${assets.glue}"></script>
</body>
</html>`;
}

/** The longest script line the page may send. */
const MAX_LINE = 120;

/**
 * The script lines the page sends: the local player sitting, standing and
 * holding an input. Anything else is dropped, so the page cannot drive any
 * other part of the world.
 */
const LINE = /^(sit you [A-Za-z_][A-Za-z0-9_]*|stand you|input you [A-Za-z]+ -?[0-9]+(\.[0-9]+)?)$/;

/** A message from the page the extension acts on. */
export type ViewMessage = { type: "ready" } | { type: "line"; line: string };

/** `msg` as a message the page may send, or `undefined`. */
export function viewMessage(msg: unknown): ViewMessage | undefined {
  if (typeof msg !== "object" || msg === null) return undefined;
  const m = msg as { type?: unknown; line?: unknown };
  if (m.type === "ready") return { type: "ready" };
  if (m.type === "line" && typeof m.line === "string" && m.line.length <= MAX_LINE && LINE.test(m.line)) {
    return { type: "line", line: m.line };
  }
  return undefined;
}

/**
 * The files a program's imports may read, as the wasm loader takes them: the
 * other `.ws` files beside it, by file name. `active` is the program's own.
 */
export function siblingFiles(entries: { name: string; text: string }[], active: string): string {
  const files: Record<string, string> = {};
  for (const { name, text } of entries) {
    if (name !== active && name.endsWith(".ws")) files[name] = text;
  }
  return JSON.stringify(files);
}

/** The module name of `fileName` (`snake.ws` -> `snake`). */
export function moduleName(fileName: string): string {
  return fileName.replace(/\.[^.]+$/, "") || fileName;
}
