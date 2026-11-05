// Runs the playground's `@test`s off the page's thread, with a compiler of its
// own: one test at a time, each report posted as it finishes, then `done`.
import init, * as wasm from '../pkg/wasm.js';

const ready = init();

self.onmessage = async ({ data }) => {
  await ready;
  const { source, moduleName, filesJson, prefabsJson, names } = data;
  for (const name of names) {
    let report;
    try {
      report = JSON.parse(wasm.wirescript_run_test(source, moduleName, filesJson, prefabsJson, name));
    } catch (e) {
      report = { name, passed: false, problems: [String(e)], lines: [] };
    }
    self.postMessage({ report });
  }
  self.postMessage({ done: true });
};
