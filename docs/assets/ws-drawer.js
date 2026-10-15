// ws-drawer.js -- a Wirescript editor in a drawer at the bottom of every book
// page: the playground's editor (Monaco, with diagnostics, completion, hover,
// go to definition, rename and format from the compiler), one buffer kept in
// this browser, and buttons to compile it to a .brz or open it in the
// playground. Each Wirescript example gets a button that loads it into the
// drawer.
//
// Like ws-snippet.js it needs the playground beside the book (the compiler at
// ../pkg/wasm.js and the editor at ../editor.js); without it, no drawer.
// Monaco comes from ws-highlight.js (window.wsLoadMonaco).
(function () {
  'use strict';

  const root = typeof path_to_root === 'string' ? path_to_root : '';
  const site = new URL(root + '../', document.baseURI);
  const NAME = 'scratch';
  const KEYS = { source: 'ws-drawer-source', open: 'ws-drawer-open', height: 'ws-drawer-height' };
  const START = [
    '// Write Wirescript here. Compile saves scratch.brz to paste in-game.',
    '@left in pressed: exec',
    'var count: int = 0',
    '@right out text = "Pressed ${count.Value} times"',
    '',
    'on pressed {',
    '  count += 1',
    '}',
    '',
  ].join('\n');

  const EDIT_ICON =
    '<span class="fa-svg"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">' +
    '<path fill="currentColor" d="M3 17.2V21h3.8l11-11-3.8-3.8-11 11zM20.7 7a1 1 0 0 0 0-1.4l-2.3-2.3a1 1 0 0 0-1.4 0l-1.8 1.8 3.8 3.8L20.7 7z"/>' +
    '</svg></span>';

  function get(key, fallback) {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : v;
    } catch {
      return fallback;
    }
  }
  function set(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Storage may be unavailable (a private window); the drawer then forgets.
    }
  }

  async function exists(url) {
    try {
      return (await fetch(url, { method: 'HEAD' })).ok;
    } catch {
      return false;
    }
  }

  async function fetchLibrary(name) {
    const resp = await fetch(new URL(root + 'learn/lib/' + name + '.ws', document.baseURI));
    if (!resp.ok) throw new Error('cannot load the library "' + name + '" (' + resp.status + ')');
    return resp.text();
  }

  function build() {
    const drawer = document.createElement('div');
    drawer.className = 'ws-drawer';
    drawer.hidden = true;
    drawer.innerHTML =
      '<div class="ws-drawer-grip" title="Drag to resize"></div>' +
      '<div class="ws-drawer-bar">' +
      '<span class="ws-drawer-title">Wirescript</span>' +
      '<span class="ws-drawer-status"></span>' +
      '<button type="button" class="ws-drawer-compile" title="Compile and download scratch.brz (Ctrl+Shift+B)">Compile</button>' +
      '<button type="button" class="ws-drawer-open" title="Open in the playground">Playground</button>' +
      '<button type="button" class="ws-drawer-close" title="Close" aria-label="Close">&times;</button>' +
      '</div>' +
      '<div class="ws-drawer-editor"></div>';
    const tab = document.createElement('button');
    tab.type = 'button';
    tab.className = 'ws-drawer-tab';
    tab.textContent = 'Editor';
    tab.title = 'Open the Wirescript editor';
    document.body.append(drawer, tab);
    // The book's own keys (arrows turn the page, S searches) must not fire
    // while typing in the editor.
    drawer.addEventListener('keydown', (e) => e.stopPropagation());
    return { drawer, tab };
  }

  async function init() {
    if (!(await exists(new URL('pkg/wasm.js', site))) || !window.wsLoadMonaco) return;
    const snippet = await import(new URL('snippet.js', site).href);
    const { drawer, tab } = build();
    const status = drawer.querySelector('.ws-drawer-status');
    const host = drawer.querySelector('.ws-drawer-editor');

    let editor = null;
    let monaco = null;
    let wasm = null;
    let libraries = {};
    let starting = null;

    function setStatus(text, kind) {
      status.textContent = text;
      status.title = text;
      status.className = 'ws-drawer-status' + (kind ? ' ws-drawer-' + kind : '');
    }

    function showDiagnostics() {
      if (!editor) return;
      const markers = monaco.editor.getModelMarkers({ resource: editor.getModel().uri });
      const errors = markers.filter((m) => m.severity === monaco.MarkerSeverity.Error).length;
      const warnings = markers.filter((m) => m.severity === monaco.MarkerSeverity.Warning).length;
      if (errors) setStatus(errors + (errors === 1 ? ' error' : ' errors'), 'error');
      else if (warnings) setStatus(warnings + (warnings === 1 ? ' warning' : ' warnings'), 'warning');
      else setStatus('No problems', '');
    }

    // Imports name libraries in learn/lib/; fetch them as the text changes so
    // the editor's checks and the compile can see them.
    let libraryTimer = null;
    function refreshLibraries() {
      clearTimeout(libraryTimer);
      libraryTimer = setTimeout(async () => {
        try {
          libraries = await snippet.collectLibraries(editor.getValue(), fetchLibrary);
        } catch {
          // An unknown import shows as a diagnostic; keep what was fetched.
        }
      }, 400);
    }

    function start() {
      if (!starting) {
        starting = (async () => {
          setStatus('Loading the editor...', '');
          const [m, w, editorModule] = await Promise.all([
            window.wsLoadMonaco(),
            import(new URL('pkg/wasm.js', site).href).then(async (mod) => {
              await mod.default();
              return mod;
            }),
            import(new URL('editor.js', site).href),
          ]);
          monaco = m;
          wasm = w;
          editor = editorModule.createEditor(host, wasm, monaco, () => JSON.stringify(libraries));
          editor.setValue(get(KEYS.source, START));
          editor.onDidChangeModelContent(() => {
            set(KEYS.source, editor.getValue());
            refreshLibraries();
          });
          editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyB, compile);
          monaco.editor.onDidChangeMarkers(showDiagnostics);
          refreshLibraries();
          showDiagnostics();
        })().catch((e) => {
          starting = null;
          setStatus('The editor could not load: ' + (e && e.message ? e.message : e), 'error');
        });
      }
      return starting;
    }

    function resize(px) {
      const h = Math.max(160, Math.min(px, window.innerHeight - 80));
      drawer.style.height = h + 'px';
      if (!drawer.hidden) document.body.style.paddingBottom = h + 'px';
      return h;
    }

    async function open() {
      drawer.hidden = false;
      tab.hidden = true;
      resize(Number(get(KEYS.height, Math.round(window.innerHeight * 0.4))));
      set(KEYS.open, '1');
      await start();
      if (editor) editor.focus();
    }

    function close() {
      drawer.hidden = true;
      tab.hidden = false;
      document.body.style.paddingBottom = '';
      set(KEYS.open, '0');
    }

    async function compile() {
      if (!editor) return;
      const button = drawer.querySelector('.ws-drawer-compile');
      button.disabled = true;
      try {
        const source = editor.getValue();
        libraries = await snippet.collectLibraries(source, fetchLibrary);
        const bytes = wasm.wirescript_compile(source, NAME, JSON.stringify(libraries), '{}');
        snippet.downloadBytes(bytes, NAME + '.brz');
        setStatus('Saved ' + NAME + '.brz', 'ok');
      } catch (e) {
        setStatus('Compile failed: ' + (e && e.message ? e.message : e), 'error');
      } finally {
        button.disabled = false;
      }
    }

    async function openInPlayground() {
      if (!editor) return;
      const win = window.open('', '_blank');
      if (win) win.opener = null;
      try {
        const source = editor.getValue();
        const files = await snippet.collectLibraries(source, fetchLibrary);
        files[NAME + '.ws'] = source;
        const share = await import(new URL('share.js', site).href);
        const href = new URL('index.html#' + (await share.encodeFiles(files, NAME + '.ws')), site).href;
        if (win) win.location.href = href;
        else window.location.href = href;
      } catch (e) {
        if (win) win.close();
        setStatus('Could not open the playground: ' + (e && e.message ? e.message : e), 'error');
      }
    }

    /** Put `text` in the drawer as one edit, so Ctrl+Z brings the old text back. */
    async function load(text) {
      await open();
      if (!editor) return;
      const model = editor.getModel();
      editor.pushUndoStop();
      editor.executeEdits('ws-drawer', [{ range: model.getFullModelRange(), text }]);
      editor.pushUndoStop();
      editor.setPosition({ lineNumber: 1, column: 1 });
      editor.revealLine(1);
    }

    tab.addEventListener('click', open);
    drawer.querySelector('.ws-drawer-close').addEventListener('click', close);
    drawer.querySelector('.ws-drawer-compile').addEventListener('click', compile);
    drawer.querySelector('.ws-drawer-open').addEventListener('click', openInPlayground);

    const grip = drawer.querySelector('.ws-drawer-grip');
    grip.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      grip.setPointerCapture(e.pointerId);
      const move = (ev) => resize(window.innerHeight - ev.clientY);
      const up = () => {
        grip.removeEventListener('pointermove', move);
        grip.removeEventListener('pointerup', up);
        set(KEYS.height, String(parseInt(drawer.style.height, 10)));
      };
      grip.addEventListener('pointermove', move);
      grip.addEventListener('pointerup', up);
    });

    for (const code of document.querySelectorAll('pre > code.language-wirescript')) {
      const pre = code.parentElement;
      let buttons = pre.querySelector('.buttons');
      if (!buttons) {
        buttons = document.createElement('div');
        buttons.className = 'buttons';
        pre.insertBefore(buttons, pre.firstChild);
      }
      const button = document.createElement('button');
      button.className = 'ws-edit-button';
      button.title = 'Edit in the drawer below';
      button.setAttribute('aria-label', button.title);
      button.innerHTML = EDIT_ICON;
      buttons.appendChild(button);
      button.addEventListener('click', () => load(snippet.preludeBefore(pre) + code.textContent.replace(/\n$/, '') + '\n'));
    }

    if (get(KEYS.open, '0') === '1') open();
  }

  const go = () => init().catch((e) => console.warn('ws-drawer: unavailable:', e));
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', go);
  else go();
})();
