import { test } from "node:test";
import * as assert from "node:assert/strict";
import { clipboardCommands } from "./clipboard";

const names = (p: NodeJS.Platform, env: NodeJS.ProcessEnv) =>
  clipboardCommands("/tmp/a.brz", p, env).map((c) => c.cmd);

test("wayland sessions try wl-copy before xclip", () => {
  assert.deepEqual(names("linux", { WAYLAND_DISPLAY: "wayland-0", DISPLAY: ":0" }), [
    "wl-copy",
    "xclip",
  ]);
});

test("plain X11 sessions try xclip before wl-copy", () => {
  assert.deepEqual(names("linux", { DISPLAY: ":0" }), ["xclip", "wl-copy"]);
});

// pathToFileURL follows the HOST's path rules, so these assert the shape of the
// uri-list (absolute file URI, percent-encoded, CRLF-terminated) rather than a
// literal string, which differs when the tests run on a Windows dev machine.
test("linux helpers take a CRLF-terminated uri-list on stdin", () => {
  for (const c of clipboardCommands("/tmp/a.brz", "linux", {})) {
    assert.ok(c.input.startsWith("file:///"), c.input);
    assert.ok(c.input.endsWith("/a.brz\r\n"), c.input);
    assert.ok(c.args.includes("text/uri-list"));
  }
});

test("paths with spaces are percent-encoded, not shell-quoted", () => {
  const [first] = clipboardCommands("/tmp/my file.brz", "linux", {});
  assert.ok(first.input.endsWith("/my%20file.brz\r\n"), first.input);
});

test("no command relies on a shell", () => {
  for (const p of ["linux", "darwin", "win32"] as NodeJS.Platform[]) {
    for (const c of clipboardCommands("/tmp/a.brz", p, {})) {
      // execFileSync runs these directly. `<<<` was a bash herestring that
      // execSync fed to /bin/sh -- a syntax error under dash (Debian/Ubuntu),
      // so the copy silently never ran there.
      assert.ok(!c.args.some((a) => a.includes("<<<")), `${c.cmd} uses a herestring`);
    }
  }
});

test("windows and macos each have exactly one helper", () => {
  assert.deepEqual(names("win32", {}), ["powershell"]);
  assert.deepEqual(names("darwin", {}), ["osascript"]);
});
