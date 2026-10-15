// ws-snippet.js -- Compile and Open-in-playground buttons on each Wirescript
// example in the book, and the Learn series' progress marks.
//
// The book is published at /docs/, beside the playground, so the playground's
// compiler (pkg/wasm.js) and its shared snippet code (snippet.js) sit one
// level above the book's root. A block is offered as a download when its fence
// names it (```wirescript name=door); the button compiles the block, its
// hidden prelude and any library it imports (from learn/lib/), and saves
// `door.brz`. Where the playground is not beside the book (`mdbook serve`),
// no button appears.
(function () {
  'use strict';

  const root = typeof path_to_root === 'string' ? path_to_root : '';
  const site = new URL(root + '../', document.baseURI);
  const ICON =
    '<span class="fa-svg"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">' +
    '<path fill="currentColor" d="M11 3h2v9.6l3.3-3.3 1.4 1.4L12 16.4l-5.7-5.7 1.4-1.4 3.3 3.3V3zM4 18h16v2H4z"/>' +
    '</svg></span>';
  const OPEN_ICON =
    '<span class="fa-svg"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">' +
    '<path fill="currentColor" d="M14 3h7v7h-2V6.4l-8.3 8.3-1.4-1.4L17.6 5H14V3zM5 5h6v2H5v12h12v-6h2v8H3V5h2z"/>' +
    '</svg></span>';

  let compiler = null;
  function loadCompiler() {
    if (!compiler) {
      compiler = import(new URL('pkg/wasm.js', site).href).then(async (wasm) => {
        await wasm.default();
        return wasm;
      });
    }
    return compiler;
  }

  async function exists(url) {
    try {
      return (await fetch(url, { method: 'HEAD' })).ok;
    } catch {
      return false;
    }
  }

  async function fetchLibrary(name) {
    const url = new URL(root + 'learn/lib/' + name + '.ws', document.baseURI);
    const resp = await fetch(url);
    if (!resp.ok) throw new Error('cannot load the library "' + name + '" (' + resp.status + ')');
    return resp.text();
  }

  function showError(pre, message) {
    let box = pre.nextElementSibling;
    if (!box || !box.classList.contains('ws-compile-error')) {
      box = document.createElement('div');
      box.className = 'ws-compile-error';
      box.style.cssText = 'color:#e06c75;white-space:pre-wrap;font-family:monospace;font-size:0.85em;margin:-0.5em 0 1em';
      pre.after(box);
    }
    box.textContent = message;
  }

  function clearError(pre) {
    const box = pre.nextElementSibling;
    if (box && box.classList.contains('ws-compile-error')) box.remove();
  }

  async function init() {
    if (!(await exists(new URL('pkg/wasm.js', site)))) return;
    const snippet = await import(new URL('snippet.js', site).href);

    for (const code of document.querySelectorAll('pre > code.language-wirescript')) {
      const rules = snippet.blockRules(snippet.wordsFromClasses(code.classList));
      if (!snippet.isCompilable(rules)) continue;
      const pre = code.parentElement;
      let buttons = pre.querySelector('.buttons');
      if (!buttons) {
        buttons = document.createElement('div');
        buttons.className = 'buttons';
        pre.insertBefore(buttons, pre.firstChild);
      }
      const button = document.createElement('button');
      button.className = 'ws-compile-button';
      button.title = 'Compile and download ' + rules.name + '.brz' + (rules.admin ? ' (needs admin to load)' : '');
      button.setAttribute('aria-label', button.title);
      button.innerHTML = ICON;
      const copy = buttons.querySelector('.clip-button');
      buttons.insertBefore(button, copy ? copy.nextSibling : buttons.firstChild);

      // Open the example, and the libraries it imports, as new playground files.
      const open = document.createElement('button');
      open.className = 'ws-open-button';
      open.title = 'Open in the playground';
      open.setAttribute('aria-label', open.title);
      open.innerHTML = OPEN_ICON;
      button.after(open);
      open.addEventListener('click', async () => {
        // Open the tab while the click still counts as the reader's, or a
        // popup blocker stops it; its address follows once the link is built.
        const tab = window.open('', '_blank');
        if (tab) tab.opener = null;
        try {
          const source = snippet.preludeBefore(pre) + code.textContent;
          const files = await snippet.collectLibraries(source, fetchLibrary);
          files[rules.name + '.ws'] = source;
          const share = await import(new URL('share.js', site).href);
          const href = new URL('index.html#' + (await share.encodeFiles(files, rules.name + '.ws')), site).href;
          if (tab) tab.location.href = href;
          else window.location.href = href;
          clearError(pre);
        } catch (e) {
          if (tab) tab.close();
          showError(pre, 'Could not open the playground: ' + (e && e.message ? e.message : e));
        }
      });

      button.addEventListener('click', async () => {
        button.disabled = true;
        try {
          const source = snippet.preludeBefore(pre) + code.textContent;
          const libraries = await snippet.collectLibraries(source, fetchLibrary);
          const wasm = await loadCompiler();
          const bytes = wasm.wirescript_compile(source, rules.name, JSON.stringify(libraries), '{}');
          snippet.downloadBytes(bytes, rules.name + '.brz');
          clearError(pre);
        } catch (e) {
          showError(pre, 'Compile failed: ' + (e && e.message ? e.message : e));
        } finally {
          button.disabled = false;
        }
      });
    }
  }

  const start = () => init().catch((e) => console.warn('ws-snippet: buttons unavailable:', e));
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();

// Learn progress: a button at the end of each lesson marks it complete, and
// the tree on the Learn page shows which lessons are done. Kept in this
// browser only; it works with or without the compiler beside the book.
(function () {
  'use strict';

  const KEY = 'ws-learn-done';
  const root = typeof path_to_root === 'string' ? path_to_root : '';
  const bookRoot = new URL(root, document.baseURI).pathname;
  // `learn/cpu/alu`, the page's path inside the book. The host may serve a
  // page with or without `.html`, so the key has none.
  const pageOf = (url) =>
    decodeURIComponent(new URL(url, document.baseURI).pathname)
      .slice(bookRoot.length)
      .replace(/\.html$/, '');

  function load() {
    try {
      return new Set(JSON.parse(localStorage.getItem(KEY) || '[]'));
    } catch {
      return new Set();
    }
  }
  function save(done) {
    try {
      localStorage.setItem(KEY, JSON.stringify([...done]));
    } catch {
      // Storage may be unavailable (a private window); progress is then per visit.
    }
  }

  function markTree(done) {
    // The graph's nodes are SVG links, whose `href` is not a string.
    for (const a of document.querySelectorAll('.learn-tree a.learn-node')) {
      a.classList.toggle('learn-done', done.has(pageOf(a.getAttribute('href'))));
    }
    for (const a of document.querySelectorAll('.learn-tree li > a:first-child')) {
      const li = a.parentElement;
      const isDone = done.has(pageOf(a.href));
      li.classList.toggle('learn-done', isDone);
      let tag = li.querySelector('.learn-done-tag');
      if (isDone && !tag) {
        tag = document.createElement('span');
        tag.className = 'learn-done-tag';
        tag.textContent = ' (done)';
        tag.style.opacity = '0.7';
        a.after(tag);
      } else if (!isDone && tag) {
        tag.remove();
      }
    }
  }

  function addButton(page, done) {
    const main = document.querySelector('main');
    if (!main) return;
    const button = document.createElement('button');
    button.className = 'learn-complete';
    button.style.cssText = 'margin:2em 0 1em;padding:0.4em 1em;cursor:pointer;font:inherit';
    const label = () => {
      button.textContent = done.has(page) ? 'Completed (click to undo)' : 'Mark this lesson complete';
    };
    button.addEventListener('click', () => {
      if (done.has(page)) done.delete(page);
      else done.add(page);
      save(done);
      label();
    });
    label();
    main.appendChild(button);
  }

  function init() {
    const done = load();
    const page = pageOf(location.href);
    if (document.querySelector('.learn-tree')) markTree(done);
    else if (page.startsWith('learn/')) addButton(page, done);
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();

// mdBook builds a folder's README.md as index.html, but turns a link to
// README.md into README.html, which does not exist. Point those links at
// index.html.
(function () {
  'use strict';

  function fix() {
    for (const a of document.querySelectorAll('a[href]')) {
      const href = a.getAttribute('href');
      if (/(^|\/)README\.html(#|$)/.test(href)) a.setAttribute('href', href.replace(/README\.html/, 'index.html'));
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', fix);
  else fix();
})();
