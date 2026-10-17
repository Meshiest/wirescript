import { test } from "node:test";
import * as assert from "node:assert/strict";
import { importedMemoryLimits } from "./wasmMemory";

/** A minimal module: a function import, then `env.memory` with the given limits. */
function moduleImporting(limits: number[]): Uint8Array {
  const name = (s: string) => [s.length, ...Buffer.from(s)];
  const imports = [
    2, // two imports
    ...name("wasi"), ...name("thread-spawn"), 0, 0, // function, type 0
    ...name("env"), ...name("memory"), 2, ...limits, // memory
  ];
  const typeSection = [1, 5, 1, 0x60, 1, 0x7f, 0]; // one type: (i32) -> ()
  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...typeSection,
    2, imports.length, ...imports,
  ]);
}

test("reads shared memory limits with a maximum", () => {
  // flags 3 (max + shared), initial 1058 (LEB 0xa2 0x08), maximum 16384 (0x80 0x80 0x01)
  assert.deepEqual(importedMemoryLimits(moduleImporting([3, 0xa2, 0x08, 0x80, 0x80, 0x01])), {
    initial: 1058,
    maximum: 16384,
  });
});

test("a memory without a maximum uses its initial size", () => {
  assert.deepEqual(importedMemoryLimits(moduleImporting([0, 5])), { initial: 5, maximum: 5 });
});

test("a module importing no memory is an error", () => {
  const bytes = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
  assert.throws(() => importedMemoryLimits(bytes), /imports no memory/);
});
