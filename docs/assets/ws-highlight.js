// ws-highlight.js -- color the book's Wirescript examples with the playground
// editor's own tokenizer (Monaco + playground/monarch.js), so a block looks the
// same in the book as in the editor.
//
// highlight.js colors every block first (wirescript-hljs.js); this then loads
// Monaco from the same CDN build the playground uses and recolors each
// `wirescript` block from Monaco's tokens. The grammar is the playground's
// `monarch.js`, one level above the book's root (the book is published at
// /docs/, beside the playground). Where it cannot load (no network, or
// `mdbook serve` with no playground beside it), the highlight.js colors stay.
// Token colors are in ws-highlight.css.
(function () {
  'use strict';

  const MONACO = 'https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/min/vs';
  const LANGUAGE_ID = 'wirescript';
  const root = typeof path_to_root === 'string' ? path_to_root : '';
  const site = new URL(root + '../', document.baseURI);

  function loadScript(src) {
    return new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => reject(new Error('cannot load ' + src));
      document.head.appendChild(s);
    });
  }

  // Shared with ws-drawer.js, so the page loads Monaco once.
  let monacoLoad = null;
  function loadMonaco() {
    if (!monacoLoad) {
      monacoLoad = (async () => {
        if (!window.require || !window.require.config) await loadScript(MONACO + '/loader.js');
        window.require.config({ paths: { vs: MONACO } });
        return new Promise((resolve, reject) => window.require(['vs/editor/editor.main'], resolve, reject));
      })();
      monacoLoad.catch(() => {
        monacoLoad = null;
      });
    }
    return monacoLoad;
  }
  window.wsLoadMonaco = loadMonaco;

  /**
   * `keyword.wirescript` -> `ws-keyword`; `number.float.wirescript` ->
   * `ws-number ws-number-float`; whitespace gets no class.
   */
  function classOf(type) {
    const parts = type.split('.').filter((p) => p && p !== LANGUAGE_ID);
    if (parts[0] === 'white') return '';
    const out = [];
    for (let i = 1; i <= parts.length; i++) out.push('ws-' + parts.slice(0, i).join('-'));
    return out.join(' ');
  }

  function escape(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function render(monaco, text) {
    const lines = text.split('\n');
    const tokens = monaco.editor.tokenize(text, LANGUAGE_ID);
    return lines
      .map((line, i) => {
        const row = tokens[i] || [];
        let html = '';
        for (let t = 0; t < row.length; t++) {
          const end = t + 1 < row.length ? row[t + 1].offset : line.length;
          const piece = line.slice(row[t].offset, end);
          if (!piece) continue;
          const cls = classOf(row[t].type);
          html += cls ? '<span class="' + cls + '">' + escape(piece) + '</span>' : escape(piece);
        }
        return html;
      })
      .join('\n');
  }

  async function init() {
    const blocks = document.querySelectorAll('pre > code.language-wirescript');
    if (!blocks.length) return;
    let monaco;
    let grammar;
    try {
      [monaco, grammar] = await Promise.all([loadMonaco(), import(new URL('monarch.js', site).href)]);
    } catch {
      return;
    }
    if (!monaco.languages.getLanguages().some((l) => l.id === LANGUAGE_ID)) {
      monaco.languages.register({ id: LANGUAGE_ID });
      monaco.languages.setMonarchTokensProvider(LANGUAGE_ID, grammar.monarchLanguage);
    }
    for (const code of blocks) {
      // Keep the trailing newline highlight.js leaves out of the tokens.
      const text = code.textContent.replace(/\n$/, '');
      code.innerHTML = render(monaco, text);
      code.classList.add('ws-monaco');
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
