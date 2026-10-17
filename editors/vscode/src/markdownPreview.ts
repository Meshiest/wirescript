import * as fs from "fs";
import * as path from "path";
import { loadWASM, OnigScanner, OnigString } from "vscode-oniguruma";
import { IGrammar, INITIAL, parseRawGrammar, Registry } from "vscode-textmate";

/**
 * The extension's own TextMate grammar (`syntaxes/wirescript.tmLanguage.json`),
 * the one that colors `.ws` files in the editor, loaded for use outside it.
 */
export async function loadGrammar(extensionPath: string): Promise<IGrammar | null> {
  // The package carries it beside the bundle (scripts/bundle.mjs); the
  // unbundled build and the tests read it from node_modules.
  const bundled = path.join(extensionPath, "dist", "onig.wasm");
  const wasm = fs.readFileSync(fs.existsSync(bundled) ? bundled : require.resolve("vscode-oniguruma/release/onig.wasm"));
  await loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const grammarPath = path.join(extensionPath, "syntaxes", "wirescript.tmLanguage.json");
  const registry = new Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns: string[]) => new OnigScanner(patterns),
      createOnigString: (text: string) => new OnigString(text),
    }),
    loadGrammar: async (scope: string) =>
      scope === "source.wirescript" ? parseRawGrammar(fs.readFileSync(grammarPath, "utf8"), grammarPath) : null,
  });
  return registry.loadGrammar("source.wirescript");
}

/**
 * The class a grammar scope gets in the preview; `media/markdown-preview.css`
 * colors these the way VS Code's default themes color `.ws` files, so the
 * docs read like the editor whatever theme the preview itself uses. `null`
 * keeps the text plain; `undefined` leaves the scope to the one enclosing it.
 */
function classFor(scope: string): string | null | undefined {
  const rules: [string, string | null][] = [
    ["comment", "ws-comment"],
    ["constant.character.escape", "ws-escape"],
    ["punctuation.definition.interpolation", "ws-escape"],
    ["string", "ws-string"],
    ["constant.numeric", "ws-number"],
    ["constant.language", "ws-constant"],
    ["keyword.operator.is", "ws-keyword"],
    ["keyword.operator", null],
    ["keyword.declaration", "ws-declaration"],
    ["keyword", "ws-keyword"],
    ["storage.modifier.annotation", "ws-annotation"],
    ["storage.modifier", "ws-declaration"],
    ["support.type", "ws-type"],
    ["entity.name.type", "ws-type"],
    ["support.function", "ws-function"],
    ["entity.name.function", "ws-function"],
    ["support.constant.event", "ws-event"],
    ["entity.name.asset", "ws-asset"],
    ["entity.name.namespace.asset", "ws-asset"],
    ["meta.reference", "ws-asset"],
    ["meta.embedded", null],
    ["punctuation", null],
    ["variable", null],
  ];
  for (const [prefix, cls] of rules) {
    if (scope === prefix || scope.startsWith(prefix + ".")) return cls;
  }
  return undefined;
}

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** `code` as HTML, each token in a span carrying its preview class. */
export function highlight(grammar: IGrammar, code: string): string {
  let state = INITIAL;
  return code
    .split("\n")
    .map((line) => {
      const { tokens, ruleStack } = grammar.tokenizeLine(line, state);
      state = ruleStack;
      return tokens
        .map((token) => {
          const text = escapeHtml(line.slice(token.startIndex, token.endIndex));
          let cls: string | null | undefined;
          for (let i = token.scopes.length - 1; i >= 0 && cls === undefined; i--) cls = classFor(token.scopes[i]);
          return cls ? `<span class="${cls}">${text}</span>` : text;
        })
        .join("");
    })
    .join("\n");
}

/**
 * Point the book's relative `.html` links in raw HTML at the markdown pages
 * they were built from (`index.html` is a folder's `README.md`), so they
 * work in the preview. Links with a scheme, a leading `/` or only a `#`
 * fragment are left alone.
 */
export function bookLinksToMarkdown(html: string): string {
  return html.replace(
    /(href=")(?![a-z][a-z0-9+.-]*:|\/|#)([^"#]*?)([^"#/]+)\.html((?:#[^"]*)?")/gi,
    (_, open: string, dir: string, name: string, rest: string) => `${open}${dir}${name === "index" ? "README" : name}.md${rest}`,
  );
}

/**
 * The docs link to the repository's `CHANGELOG.md` by its path from the book
 * source (`docs/src`), which leaves the bundled copy. Point such a link, in
 * the page `docFile`, at the changelog copied into `docsDir`; any other link
 * comes back unchanged.
 */
export function repoLinkToDocs(href: string, docFile: string, docsDir: string): string {
  const [target, fragment] = href.split(/(?=#)/);
  if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith("/")) return href;
  const resolved = path.resolve(path.dirname(docFile), decodeURI(target));
  const repoChangelog = path.resolve(docsDir, "..", "..", "CHANGELOG.md");
  if (resolved.toLowerCase() !== repoChangelog.toLowerCase()) return href;
  const local = path.relative(path.dirname(docFile), path.join(docsDir, "CHANGELOG.md")).split(path.sep).join("/");
  return local + (fragment ?? "");
}

/**
 * A markdown-it plugin for the built-in markdown preview. ```wirescript blocks
 * in any markdown are colored with `grammar` (everything else goes to the
 * preview's own highlighter). In the bundled docs under `docsDir`, the book's
 * `.html` links in raw HTML (the Learn page's lesson tree) open the markdown
 * pages instead, and links to the repository's changelog open the bundled one.
 */
export function wirescriptMarkdown(grammar: IGrammar | null, docsDir: string) {
  const inDocs = (env: any) => {
    const doc: string | undefined = env?.currentDocument?.fsPath;
    return !!doc && path.resolve(doc).toLowerCase().startsWith(path.resolve(docsDir).toLowerCase() + path.sep);
  };
  return (md: any) => {
    const previous = md.options.highlight;
    md.options.highlight = (code: string, lang: string, attrs: string) => {
      if (grammar && (lang === "wirescript" || lang === "ws")) return highlight(grammar, code);
      return previous ? previous(code, lang, attrs) : "";
    };
    for (const rule of ["html_block", "html_inline"]) {
      const render = md.renderer.rules[rule];
      md.renderer.rules[rule] = (tokens: any[], idx: number, options: any, env: any, self: any) => {
        const html = render ? render(tokens, idx, options, env, self) : tokens[idx].content;
        return inDocs(env) ? bookLinksToMarkdown(html) : html;
      };
    }
    const renderLink = md.renderer.rules.link_open;
    md.renderer.rules.link_open = (tokens: any[], idx: number, options: any, env: any, self: any) => {
      const href = tokens[idx].attrGet("href");
      if (href && inDocs(env)) tokens[idx].attrSet("href", repoLinkToDocs(href, env.currentDocument.fsPath, docsDir));
      return renderLink ? renderLink(tokens, idx, options, env, self) : self.renderToken(tokens, idx, options);
    };
    return md;
  };
}
