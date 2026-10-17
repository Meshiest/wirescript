import {
  commands,
  env,
  ExtensionContext,
  TreeDataProvider,
  TreeItem,
  TreeItemCollapsibleState,
  Uri,
  window,
  workspace,
} from "vscode";
import { DocNode, flattenPages, parseSummary } from "./docsIndex";

const ONLINE_DOCS = "https://wirescript.brickadia.dev/docs";

/**
 * The language docs, bundled in the extension's `docs/` folder: a "Wirescript
 * Docs" tree in the Explorer and a searchable "Open Documentation" command.
 * Pages open in the built-in markdown preview, which follows the links
 * between them.
 */
export function registerDocs(context: ExtensionContext) {
  const docsDir = Uri.joinPath(context.extensionUri, "docs");
  let index: Promise<DocNode[]> | undefined;
  const load = () =>
    (index ??= Promise.resolve(workspace.fs.readFile(Uri.joinPath(docsDir, "SUMMARY.md"))).then(
      (bytes) => parseSummary(new TextDecoder().decode(bytes)),
      () => [],
    ));
  // A page opens in the preview, at `fragment` when given; one not bundled
  // opens on the website.
  const openPage = async (path: string, fragment?: string) => {
    const page = Uri.joinPath(docsDir, path);
    try {
      await workspace.fs.stat(page);
    } catch {
      const online = path.replace(/(^|\/)README\.md$/, "$1index.md").replace(/\.md$/, ".html");
      return env.openExternal(Uri.parse(`${ONLINE_DOCS}/${online}${fragment ? `#${fragment}` : ""}`));
    }
    return commands.executeCommand("markdown.showPreview", fragment ? page.with({ fragment }) : page);
  };

  const tree: TreeDataProvider<DocNode> = {
    getTreeItem(node) {
      const item = new TreeItem(
        node.title,
        node.children.length > 0 ? TreeItemCollapsibleState.Collapsed : TreeItemCollapsibleState.None,
      );
      if (node.path) {
        item.command = { command: "wirescript.openDocsPage", title: "Open", arguments: [node.path] };
        item.tooltip = node.path;
      }
      return item;
    },
    getChildren(node) {
      return node ? node.children : load();
    },
  };

  context.subscriptions.push(
    window.registerTreeDataProvider("wirescript.docs", tree),
    commands.registerCommand("wirescript.openDocsPage", (path: string, fragment?: string) => openPage(path, fragment)),
    commands.registerCommand("wirescript.openDocs", async () => {
      const pages = flattenPages(await load());
      if (pages.length === 0) {
        const choice = await window.showWarningMessage("The Wirescript docs are not bundled with this build.", "Open Online");
        if (choice) env.openExternal(Uri.parse(ONLINE_DOCS));
        return;
      }
      const pick = await window.showQuickPick(
        pages.map((p) => ({ label: p.title, description: p.trail.join(" / "), path: p.path })),
        { placeHolder: "Search the Wirescript docs", matchOnDescription: true },
      );
      if (pick) openPage(pick.path);
    }),
  );
}
