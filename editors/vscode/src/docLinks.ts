/**
 * The server links hovers and diagnostics to the published docs. These map
 * such a link to the bundled copy, opened by `wirescript.openDocsPage`.
 */
export const DOCS_URL = "https://wirescript.brickadia.dev/docs/";

export const OPEN_DOCS_PAGE = "wirescript.openDocsPage";

/**
 * The bundled page (`builtins.md`, a folder's `index.html` as its
 * `README.md`) and section a docs URL names, as `openDocsPage`'s arguments,
 * or `undefined` for any other URL.
 */
export function bundledPage(url: string): [string] | [string, string] | undefined {
  if (!url.startsWith(DOCS_URL)) return undefined;
  const [path, fragment] = url.slice(DOCS_URL.length).split("#", 2);
  if (!/\.html$/.test(path)) return undefined;
  const page = path.replace(/(^|\/)index\.html$/, "$1README.md").replace(/\.html$/, ".md");
  return fragment ? [page, decodeURIComponent(fragment)] : [page];
}

/** A `command:` URI that opens the bundled page a docs URL names. */
export function openPageCommand(url: string): string | undefined {
  const args = bundledPage(url);
  return args && `command:${OPEN_DOCS_PAGE}?${encodeURIComponent(JSON.stringify(args))}`;
}

/** `markdown` with each docs link pointed at the bundled page. */
export function bundleDocLinks(markdown: string): string {
  return markdown.replace(/\]\((https:\/\/wirescript\.brickadia\.dev\/docs\/[^)\s]*)\)/g, (link, url: string) => {
    const command = openPageCommand(url);
    return command ? `](${command})` : link;
  });
}

/**
 * A URI for the extension's own URI handler (`base` is
 * `<uriScheme>://<extension id>`) that opens the bundled page a docs URL
 * names. Diagnostics link with this rather than a `command:` URI, which the
 * editor's problem hover does not run.
 */
export function handlerUri(url: string, base: string): string | undefined {
  const args = bundledPage(url);
  if (!args) return undefined;
  const [page, fragment] = args;
  return `${base}/docs?page=${encodeURIComponent(page)}${fragment ? `#${encodeURIComponent(fragment)}` : ""}`;
}
