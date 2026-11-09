// The playground runner's simulation, off the page's thread: the shared loop
// of runner-core.js around the wasm `Runner`.
import init, * as wasm from '../pkg/wasm.js';
import { serveRunner } from './runner-core.js';

const ready = init();
const queued = [];
self.onmessage = (event) => queued.push(event);
ready.then(() => {
  serveRunner(self, wasm.Runner);
  for (const event of queued.splice(0)) self.onmessage(event);
});
