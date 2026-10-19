import { OutputChannel, Uri, workspace } from "vscode";
import { ProcessOptions, Wasm, WasmProcess } from "@vscode/wasm-wasi/v1";
import { createStdioOptions, startServer } from "@vscode/wasm-wasi-lsp";
import type { MessageTransports } from "vscode-languageclient";
import { asBuffers } from "./bufferStream";
import { importedMemoryLimits } from "./wasmMemory";

/**
 * Run the language server compiled to WASI (`wasm32-wasip1-threads`) on the
 * WebAssembly Execution Engine extension, with the workspace folders mounted
 * as wasiPaths.ts describes. Its stderr goes to `channel`. `onProcess` gets the
 * process, which the caller terminates when it stops the client: the host
 * never closes the server's stdin, so the server cannot see the client go.
 * `onExit` hears when the process ends, for whatever reason: the language
 * client only sees a crash as a stream error and keeps talking to the dead
 * server, so restarting is up to the caller.
 */
export async function startWasiServer(
  wasmUri: Uri,
  channel: OutputChannel,
  onProcess: (process: WasmProcess) => void,
  onExit: (process: WasmProcess, reason: string) => void,
): Promise<MessageTransports> {
  const wasm = await Wasm.load();
  const bits = await workspace.fs.readFile(wasmUri);
  // readFile hands back an ordinary ArrayBuffer-backed array.
  const module = await WebAssembly.compile(bits as Uint8Array<ArrayBuffer>);
  const options: ProcessOptions = {
    stdio: createStdioOptions(),
    mountPoints: [{ kind: "workspaceFolder" }],
  };
  const process = await wasm.createProcess(
    "wirescript-lsp",
    module,
    { ...importedMemoryLimits(bits), shared: true },
    options,
  );
  onProcess(process);
  const decoder = new TextDecoder("utf-8");
  process.stderr?.onData((data) => channel.append(decoder.decode(data)));
  // `startServer` calls `run()` and keeps the result to itself, so wrap it to
  // see the exit too.
  const run = process.run.bind(process);
  process.run = () => {
    const ran = run();
    ran.then(
      (code) => onExit(process, `exited with code ${code}`),
      (e) => onExit(process, `crashed: ${e?.message ?? e}`),
    );
    return ran;
  };
  return startServer(process, process.stdout && asBuffers(process.stdout));
}
