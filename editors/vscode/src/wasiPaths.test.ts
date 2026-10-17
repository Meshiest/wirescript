import { test } from "node:test";
import * as assert from "node:assert/strict";
import { translateUri, wasiMounts } from "./wasiPaths";

const root = "file:///c%3A/dev";

test("one folder mounts at /workspace", () => {
  const pairs = wasiMounts([{ uri: `${root}/ws`, name: "ws" }]);
  assert.equal(translateUri(`${root}/ws/src/a.ws`, pairs, 0), "file:///workspace/src/a.ws");
  assert.equal(translateUri("file:///workspace/src/a.ws", pairs, 1), `${root}/ws/src/a.ws`);
});

test("several folders mount at /workspaces/<name>", () => {
  const pairs = wasiMounts([
    { uri: `${root}/wirescript`, name: "wirescript" },
    { uri: `${root}/wirescript-projects`, name: "wirescript-projects" },
    { uri: `${root}/my code`, name: "my code" },
  ]);
  assert.equal(translateUri(`${root}/wirescript/a.ws`, pairs, 0), "file:///workspaces/wirescript/a.ws");
  assert.equal(translateUri(`${root}/wirescript-projects/a.ws`, pairs, 0), "file:///workspaces/wirescript-projects/a.ws");
  assert.equal(translateUri(`${root}/my code/a.ws`, pairs, 0), "file:///workspaces/my%20code/a.ws");
  assert.equal(translateUri("file:///workspaces/wirescript-projects/a.ws", pairs, 1), `${root}/wirescript-projects/a.ws`);
  // Outside every folder: unchanged.
  assert.equal(translateUri(`${root}/other/a.ws`, pairs, 0), `${root}/other/a.ws`);
});

test("a nested folder wins over its parent", () => {
  const pairs = wasiMounts([
    { uri: `${root}/repo`, name: "repo" },
    { uri: `${root}/repo/sub`, name: "sub" },
  ]);
  assert.equal(translateUri(`${root}/repo/sub/a.ws`, pairs, 0), "file:///workspaces/sub/a.ws");
});
