// The runner's simulation in the extension host, off its main thread: the
// shared loop of runner-core.js around the wasm `Runner` (the Node build).
import { parentPort } from 'node:worker_threads';
import { serveRunner } from '../../../crates/wasm/playground/runner-core.js';
import wasm from './wasm.js';

serveRunner(parentPort, wasm.Runner);
