// The runner webview's page: the shared view, relaying to the extension,
// which runs the simulation in a worker of its own.
import { createRunnerView } from './runner-view.js';

const vscode = acquireVsCodeApi();
const view = createRunnerView(document.getElementById('runner'), (msg) => vscode.postMessage(msg));
window.addEventListener('message', (event) => view.handle(event.data));
vscode.postMessage({ type: 'ready' });
