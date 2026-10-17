/** One entry of the docs' table of contents. */
export interface DocNode {
  title: string;
  /** The page, relative to the docs folder; absent for a heading with no page. */
  path?: string;
  children: DocNode[];
}

/**
 * Parse mdBook's `SUMMARY.md`: one `[Title](path)` per line (optionally a
 * `- ` list item), nested by indentation, `[Title]()` for a heading with no
 * page of its own.
 */
export function parseSummary(text: string): DocNode[] {
  const roots: DocNode[] = [];
  const open: { indent: number; node: DocNode }[] = [];
  for (const line of text.split(/\r?\n/)) {
    const m = /^(\s*)(?:[-*] )?\[([^\]]+)\]\(([^)]*)\)/.exec(line);
    if (!m) continue;
    const indent = m[1].length;
    const node: DocNode = { title: m[2], path: m[3] || undefined, children: [] };
    while (open.length > 0 && open[open.length - 1].indent >= indent) open.pop();
    (open.length > 0 ? open[open.length - 1].node.children : roots).push(node);
    open.push({ indent, node });
  }
  return roots;
}

/** Every page in `nodes`, depth first, with the titles of the headings above it. */
export function flattenPages(nodes: DocNode[], trail: string[] = []): { title: string; path: string; trail: string[] }[] {
  const out: { title: string; path: string; trail: string[] }[] = [];
  for (const node of nodes) {
    if (node.path) out.push({ title: node.title, path: node.path, trail });
    out.push(...flattenPages(node.children, [...trail, node.title]));
  }
  return out;
}
