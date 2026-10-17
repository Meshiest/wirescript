import type { Readable } from "@vscode/wasm-wasi/v1";

/**
 * `readable` with each chunk handed on as a `Buffer` over the same bytes.
 *
 * The WASI host emits plain `Uint8Array`s. vscode-jsonrpc's Node message
 * buffer slices a chunk with `Buffer.from(chunk, 0, length)`, which for a
 * `Uint8Array` copies the whole chunk and ignores the length, so a chunk that
 * holds more than one frame (a header with the end of the previous body)
 * reads as garbage: "Header must provide a Content-Length property". For a
 * `Buffer` it takes the `Buffer.slice` path, which is right.
 */
export function asBuffers(readable: Readable): Readable {
  return {
    pause: (flush) => readable.pause(flush),
    resume: () => readable.resume(),
    onData: (listener, thisArgs, disposables) =>
      readable.onData(
        (chunk) => listener.call(thisArgs, Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength)),
        undefined,
        disposables,
      ),
  };
}
