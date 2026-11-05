// VS Code's Testing view for Wirescript: every `@test` (fn, mod or named
// chip) in the workspace's `.ws` files, and every program with a `.expected`
// file beside it, run in the language server's gate simulator.
//
// The tree is file > test. A file item resolves its children by asking the
// server (`wirescript.listTests`); running an item asks it for a verdict
// (`wirescript.runTest`). Saving, creating or deleting a `.ws`, `.expected`
// or `.script` file refreshes that file's tests.
import {
  CancellationToken,
  CancellationTokenSource,
  ExtensionContext,
  Location,
  Position,
  Range,
  RelativePattern,
  TestController,
  TestItem,
  TestMessage,
  TestRunProfileKind,
  TestRunRequest,
  Uri,
  tests,
  workspace,
} from "vscode";
import { EXPECTED, Verdict, failureText, outputLine, testId, testName } from "./testIds";

/** Sends a `workspace/executeCommand` to the running language server. */
export type ServerCommand = (command: string, args: unknown[]) => Promise<unknown>;

/** What `wirescript.listTests` returns for a file. */
export interface TestList {
  tests: { name: string; kind: string; range: LspRange }[];
  expected: boolean;
}

interface LspRange {
  start: { line: number; character: number };
  end: { line: number; character: number };
}

/** The command a "Run test" lens runs: `[uri, test name or null]`. */
export const RUN_TEST_ITEM = "wirescript.runTestItem";

const WS_GLOB = "**/*.ws";
const IGNORED = "**/{node_modules,target,.git,.scratch}/**";

export function registerTests(
  context: ExtensionContext,
  command: () => ServerCommand | undefined,
  toServer: () => (uri: Uri) => string,
): {
  controller: TestController;
  discover: () => Promise<void>;
  runTest: (uri: Uri, name: string | null) => Promise<void>;
} {
  const ctrl = tests.createTestController("wirescript", "Wirescript");
  context.subscriptions.push(ctrl);

  const fileItem = (uri: Uri): TestItem => {
    const existing = ctrl.items.get(uri.toString());
    if (existing) return existing;
    const item = ctrl.createTestItem(uri.toString(), workspace.asRelativePath(uri), uri);
    item.canResolveChildren = true;
    ctrl.items.add(item);
    return item;
  };

  // Ask the server what a file holds and mirror it under the file's item; a
  // file with nothing to run leaves the tree.
  const refresh = async (uri: Uri): Promise<void> => {
    const send = command();
    if (!send) return;
    let list: TestList;
    try {
      list = (await send("wirescript.listTests", [toServer()(uri)])) as TestList;
    } catch {
      return;
    }
    if (!list || (list.tests.length === 0 && !list.expected)) {
      ctrl.items.delete(uri.toString());
      return;
    }
    const file = fileItem(uri);
    const children: TestItem[] = [];
    if (list.expected) {
      children.push(ctrl.createTestItem(testId(uri.toString(), EXPECTED), EXPECTED, uri));
    }
    for (const t of list.tests) {
      const item = ctrl.createTestItem(testId(uri.toString(), t.name), t.name, uri);
      item.description = `@test ${t.kind}`;
      item.range = new Range(
        new Position(t.range.start.line, t.range.start.character),
        new Position(t.range.end.line, t.range.end.character),
      );
      children.push(item);
    }
    file.children.replace(children);
    file.canResolveChildren = false;
  };

  const discoverAll = async (): Promise<void> => {
    const files = await workspace.findFiles(WS_GLOB, IGNORED);
    await Promise.all(files.map(refresh));
  };

  ctrl.resolveHandler = async (item) => {
    if (item) await refresh(item.uri!);
    else await discoverAll();
  };
  ctrl.refreshHandler = async () => {
    ctrl.items.replace([]);
    await discoverAll();
  };

  // A `.ws` file and the `.expected`/`.script` beside it are one test file.
  const wsOf = (uri: Uri): Uri => uri.with({ path: uri.path.replace(/\.(expected|script)$/, ".ws") });
  for (const folder of workspace.workspaceFolders ?? []) {
    const watcher = workspace.createFileSystemWatcher(
      new RelativePattern(folder, "**/*.{ws,expected,script}"),
    );
    watcher.onDidCreate((u) => void refresh(wsOf(u)));
    watcher.onDidChange((u) => void refresh(wsOf(u)));
    watcher.onDidDelete((u) => {
      if (u.path.endsWith(".ws")) ctrl.items.delete(u.toString());
      else void refresh(wsOf(u));
    });
    context.subscriptions.push(watcher);
  }
  context.subscriptions.push(
    workspace.onDidSaveTextDocument((doc) => {
      if (doc.languageId === "wirescript") void refresh(doc.uri);
    }),
  );

  const run = (race: number) => async (request: TestRunRequest, token: CancellationToken) => {
    const testRun = ctrl.createTestRun(request);
    const queue: TestItem[] = [];
    const excluded = new Set((request.exclude ?? []).map((t) => t.id));
    const add = async (item: TestItem): Promise<void> => {
      if (excluded.has(item.id)) return;
      if (item.uri && item.canResolveChildren) await refresh(item.uri);
      if (item.children.size > 0) item.children.forEach((c) => void queue.push(c));
      else if (item.parent) queue.push(item);
    };
    if (request.include) {
      for (const item of request.include) await add(item);
    } else {
      if (ctrl.items.size === 0) await discoverAll();
      const roots: TestItem[] = [];
      ctrl.items.forEach((i) => void roots.push(i));
      for (const item of roots) await add(item);
    }
    for (const item of queue) {
      if (!excluded.has(item.id)) testRun.enqueued(item);
    }

    for (const item of queue) {
      if (excluded.has(item.id)) continue;
      if (token.isCancellationRequested) {
        testRun.skipped(item);
        continue;
      }
      const send = command();
      if (!send) {
        testRun.errored(item, new TestMessage("The Wirescript language server is not running."));
        continue;
      }
      testRun.started(item);
      const started = Date.now();
      let verdict: Verdict;
      try {
        verdict = (await send("wirescript.runTest", [
          toServer()(item.uri!),
          testName(item.id),
          { race, quiet: true },
        ])) as Verdict;
      } catch (err: any) {
        testRun.errored(item, new TestMessage(String(err?.message ?? err)), Date.now() - started);
        continue;
      }
      for (const line of verdict.lines ?? []) {
        testRun.appendOutput(outputLine(item.label, line.tick, line.text), undefined, item);
      }
      const duration = Date.now() - started;
      if (verdict.passed) {
        testRun.passed(item, duration);
      } else {
        const message = new TestMessage(failureText(verdict));
        if (item.uri && item.range) message.location = new Location(item.uri, item.range);
        testRun.failed(item, message, duration);
      }
    }
    testRun.end();
  };

  const runProfile = ctrl.createRunProfile("Run", TestRunProfileKind.Run, run(0), true);
  ctrl.createRunProfile("Run under 50 handler orders (race mode)", TestRunProfileKind.Run, run(50), false);

  // One test run as the gutter button runs it, so its result marks the
  // gutter and the Testing view: what a "Run test" lens does.
  const runTest = async (uri: Uri, name: string | null): Promise<void> => {
    await refresh(uri);
    const item = ctrl.items.get(uri.toString())?.children.get(testId(uri.toString(), name ?? EXPECTED));
    if (!item) return;
    const source = new CancellationTokenSource();
    try {
      await run(0)(new TestRunRequest([item], undefined, runProfile), source.token);
    } finally {
      source.dispose();
    }
  };
  return { controller: ctrl, discover: discoverAll, runTest };
}
