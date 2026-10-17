/**
 * Where the WebAssembly Execution Engine mounts each workspace folder for a
 * WASI process, as `[folder URI, URI the server sees]`: a lone folder at
 * `/workspace`, several at `/workspaces/<name>`. `@vscode/wasm-wasi-lsp`'s own
 * `createUriConverters` maps several folders to `/workspace/<name>`, which
 * the engine never mounts, so a multi-root window's imports did not resolve.
 */
export function wasiMounts(folders: readonly { uri: string; name: string }[]): [string, string][] {
  if (folders.length === 1) return [[folders[0].uri, "file:///workspace"]];
  return folders.map((f) => [f.uri, `file:///workspaces/${encodeURIComponent(f.name)}`]);
}

/**
 * `uri` with the longest mount prefix in `pairs` swapped from side `from` to
 * the other side. A prefix matches whole path segments only, so a folder
 * `…/wirescript` does not claim `…/wirescript-projects`. A URI under no mount
 * comes back unchanged.
 */
export function translateUri(uri: string, pairs: [string, string][], from: 0 | 1): string {
  let best: [string, string] | undefined;
  for (const pair of pairs) {
    const prefix = pair[from];
    if ((uri === prefix || uri.startsWith(prefix + "/")) && (!best || prefix.length > best[from].length)) best = pair;
  }
  return best ? best[1 - from] + uri.slice(best[from].length) : uri;
}
