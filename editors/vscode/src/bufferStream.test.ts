import { test } from "node:test";
import * as assert from "node:assert/strict";
import { Emitter, ReadableStreamMessageReader } from "vscode-jsonrpc/node";
import type { Readable } from "@vscode/wasm-wasi/v1";
import { asBuffers } from "./bufferStream";

const frame = (message: object) => {
  const body = JSON.stringify({ jsonrpc: "2.0", ...message });
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`;
};

test("reads frames that share a chunk", async () => {
  const data = new Emitter<Uint8Array>();
  const host = { pause() {}, resume() {}, onData: data.event } as unknown as Readable;
  // The rest of the stream wrapper @vscode/wasm-wasi-lsp puts around it.
  const none = () => ({ dispose() {} });
  const reader = new ReadableStreamMessageReader({ onData: asBuffers(host).onData, onError: none, onClose: none, onEnd: none } as any);
  const methods: string[] = [];
  const errors: string[] = [];
  reader.onError((e) => errors.push(String(e)));
  reader.listen((m: any) => methods.push(m.method ?? `id ${m.id}`));
  // What the server's line-buffered stdout hands the host: everything up to
  // the last newline (one frame and the next header), then the next body.
  const all = frame({ id: 1, result: null }) + frame({ method: "a", params: {} }) + frame({ method: "é", params: {} });
  const cut = all.lastIndexOf("\r\n\r\n") + 4;
  const bytes = new TextEncoder().encode(all);
  const at = Buffer.byteLength(all.slice(0, cut));
  data.fire(bytes.subarray(0, at));
  data.fire(bytes.subarray(at));
  // The reader hands messages on asynchronously.
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(errors, []);
  assert.deepEqual(methods, ["id 1", "a", "é"]);
});
