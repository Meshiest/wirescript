// docs.js -- Documentation panel for the Wirescript playground

// Playground help page is kept inline; all other pages are fetched from docs/*.md
const DOCS_CACHE = {
  Playground: `# Wirescript Playground

A browser-based development environment for Wirescript with full language support.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| **Ctrl+Shift+B** | Compile to .brz and download |
| **Shift+Alt+F** | Format document |
| **Ctrl+Shift+S** | Share (copy URL to clipboard) |
| **Ctrl+Space** | Trigger completions |
| **Ctrl+/** | Toggle line comment |
| **F12** | Go to definition |
| **Shift+F12** | Find all references |
| **F2** | Rename symbol |
| **Ctrl+Z** / **Ctrl+Y** | Undo / Redo |
| **Ctrl+D** | Select next occurrence |
| **Ctrl+Shift+K** | Delete line |

## Features

### Editor
- **Syntax highlighting** for all Wirescript keywords, types, builtins, strings, and operators
- **Live error diagnostics** — parser and type errors shown as you type (red/yellow squiggles)
- **Autocomplete** — keywords, builtin functions, types, and your declared symbols. Inside function calls, named parameters are suggested.
- **Hover information** — hover any symbol to see its type. Builtin functions show gate documentation with parameter descriptions.
- **Go to definition** — F12 or Ctrl+Click to jump to a symbol's declaration
- **Find references** — Shift+F12 to find all usages of a symbol
- **Rename** — F2 to rename a symbol across the file
- **Format** — Shift+Alt+F to auto-indent and clean up whitespace

### File Management
- Multiple \`.ws\` files stored in your browser's localStorage
- Click a file in the sidebar to switch, right-click for rename/delete
- The **+ New** button creates a new file
- Files persist across browser sessions

### Compile
- Click **Compile** or press **Ctrl+Shift+B** to compile the current file
- Downloads a \`.brz\` file that can be loaded in Brickadia via \`BR.World.LoadAdditive\`

### Share
- Click **Share** or press **Ctrl+Shift+S**
- The current source is compressed and encoded into the URL hash
- Share the URL — anyone opening it gets the code loaded in their editor

### Documentation
- Click **Docs** to toggle this panel
- Browse the full language reference
- Click **Insert Example** on code blocks to paste them into the editor
`,
};

// Pages fetched from docs/*.md
const DOC_MANIFEST = {
  Overview: 'docs/README.md',
  Syntax: 'docs/syntax.md',
  Types: 'docs/types.md',
  Expressions: 'docs/expressions.md',
  Statements: 'docs/statements.md',
  'Chips & Mods': 'docs/chips.md',
  Builtins: 'docs/builtins.md',
  'Exec Context': 'docs/exec-context.md',
};

// List of doc pages in display order
export const DOC_PAGES = [
  'Playground',
  'Overview',
  'Syntax',
  'Types',
  'Expressions',
  'Statements',
  'Chips & Mods',
  'Builtins',
  'Exec Context',
];

// Display names for the navigation
export const DOC_TITLES = {
  Playground: 'Playground Help',
  Overview: 'Overview',
  Syntax: 'Syntax',
  Types: 'Types',
  Expressions: 'Expressions',
  Statements: 'Statements',
  'Chips & Mods': 'Chips & Mods',
  Builtins: 'Built-in Functions',
  'Exec Context': 'Execution Context',
};

async function loadDoc(page) {
  if (DOCS_CACHE[page] !== undefined) return DOCS_CACHE[page];
  const path = DOC_MANIFEST[page];
  if (!path) return `# ${page}\n\nPage not found.`;
  try {
    const resp = await fetch(path);
    if (resp.ok) {
      const md = await resp.text();
      DOCS_CACHE[page] = md;
      return md;
    }
  } catch (e) { /* network error */ }
  const msg = `# ${page}\n\nFailed to load documentation.`;
  DOCS_CACHE[page] = msg;
  return msg;
}

/**
 * Initialize the docs panel.
 * @param {HTMLElement} container - The docs panel container
 * @param {Function} onInsertExample - Callback when "Insert Example" is clicked, receives code string
 * @returns {{ showPage: (name: string) => void, getCurrentPage: () => string }}
 */
export function initDocs(container, onInsertExample) {
  let currentPage = 'Overview';

  // Create navigation and content areas
  const nav = document.createElement('div');
  nav.className = 'docs-nav';

  const content = document.createElement('div');
  content.className = 'docs-content';

  container.appendChild(nav);
  container.appendChild(content);

  // Build navigation buttons
  function buildNav() {
    nav.innerHTML = '';
    for (const page of DOC_PAGES) {
      const btn = document.createElement('button');
      btn.className = 'docs-nav-btn' + (page === currentPage ? ' active' : '');
      btn.textContent = DOC_TITLES[page] || page;
      btn.addEventListener('click', () => showPage(page));
      nav.appendChild(btn);
    }
  }

  async function showPage(name) {
    currentPage = name;
    buildNav();
    await renderContent(name);
  }

  async function renderContent(name) {
    const md = await loadDoc(name);

    // Use marked if available, otherwise basic rendering
    if (typeof marked !== 'undefined') {
      marked.setOptions({ gfm: true, breaks: false });
      content.innerHTML = marked.parse(md);
    } else {
      // Very basic fallback
      content.innerHTML = '<pre>' + escapeHtml(md) + '</pre>';
    }

    // Syntax-highlight wirescript code blocks via Monaco
    const codeBlocks = content.querySelectorAll('pre code');
    codeBlocks.forEach(block => {
      const cls = block.className || '';
      const isWs =
        cls.includes('wirescript') ||
        cls.includes('language-wirescript') ||
        !cls.includes('language-');
      if (isWs && typeof monaco !== 'undefined') {
        block.setAttribute('data-lang', 'wirescript');
        monaco.editor.colorizeElement(block, { theme: 'wirescript-dark' });
      }
    });

    // Add "Insert Example" buttons to code blocks
    codeBlocks.forEach(block => {
      const pre = block.parentElement;
      const cls = block.className || '';
      if (
        cls.includes('wirescript') ||
        cls.includes('language-wirescript') ||
        !cls.includes('language-')
      ) {
        const btn = document.createElement('button');
        btn.className = 'docs-insert-btn';
        btn.textContent = 'Insert Example';
        btn.addEventListener('click', () => {
          if (onInsertExample) {
            onInsertExample(block.innerText.replace(/\u00a0/g, ' '));
          }
        });
        // Wrap pre in a container for positioning
        const wrapper = document.createElement('div');
        wrapper.className = 'docs-code-wrapper';
        pre.parentNode.insertBefore(wrapper, pre);
        wrapper.appendChild(pre);
        wrapper.appendChild(btn);
      }
    });

    // Intercept doc links — navigate within the panel instead of opening new tabs
    content.querySelectorAll('a[href]').forEach(a => {
      const href = a.getAttribute('href');
      if (!href) return;
      const basename = href.replace(/^.*\//, '');
      for (const [page, path] of Object.entries(DOC_MANIFEST)) {
        if (path.endsWith(basename)) {
          a.addEventListener('click', (e) => { e.preventDefault(); showPage(page); });
          a.removeAttribute('target');
          return;
        }
      }
    });

    // Scroll to top
    content.scrollTop = 0;
  }

  function escapeHtml(str) {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  async function showPageAndScroll(page, searchText) {
    await showPage(page);
    if (!searchText) return;
    const text = searchText.toLowerCase();
    const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode;
      if (node.textContent.toLowerCase().includes(text)) {
        const el = node.parentElement;
        if (el) {
          el.scrollIntoView({ behavior: 'smooth', block: 'center' });
          const mark = document.createElement('mark');
          mark.style.background = '#613214';
          mark.style.color = '#d4d4d4';
          const idx = node.textContent.toLowerCase().indexOf(text);
          const range = document.createRange();
          range.setStart(node, idx);
          range.setEnd(node, idx + searchText.length);
          range.surroundContents(mark);
          setTimeout(() => {
            if (mark.parentNode) {
              mark.replaceWith(mark.textContent);
            }
          }, 3000);
        }
        break;
      }
    }
  }

  let searchIndex = null;

  async function loadSearchIndex() {
    if (searchIndex) return searchIndex;
    try {
      const resp = await fetch('docs/search-index.json');
      if (resp.ok) { searchIndex = await resp.json(); return searchIndex; }
    } catch {}
    return null;
  }

  async function search(query) {
    if (!query || query.length < 2) return [];
    const q = query.toLowerCase();

    const idx = await loadSearchIndex();
    if (idx) {
      const words = q.split(/\s+/).filter(w => w.length >= 2);
      const scored = [];
      const seen = new Set();
      for (const e of idx) {
        let score = 0;
        for (const w of words) {
          for (const [kw, s] of Object.entries(e.k)) {
            if (kw === w) score += s;
            else if (kw.startsWith(w) || w.startsWith(kw)) score += s * 0.5;
          }
          if (e.t.toLowerCase().includes(w)) score += 1;
        }
        if (score <= 0) continue;
        const dedupKey = e.t.substring(0, 60) + '|' + e.p;
        if (seen.has(dedupKey)) continue;
        seen.add(dedupKey);
        scored.push({ page: e.p, line: e.t, title: e.s, score });
      }
      scored.sort((a, b) => b.score - a.score);
      return scored.slice(0, 20);
    }

    // Fallback: raw markdown scan
    await Promise.all(DOC_PAGES.map(p => loadDoc(p)));
    const results = [];
    for (const page of DOC_PAGES) {
      const text = DOCS_CACHE[page] || '';
      const lines = text.split('\n');
      const headers = [];
      for (let i = 0; i < lines.length; i++) {
        const hMatch = lines[i].match(/^(#{1,4})\s+(.+)/);
        if (hMatch) {
          const level = hMatch[1].length;
          const heading = hMatch[2].replace(/[`*_|\\]/g, '').trim();
          while (headers.length >= level) headers.pop();
          headers.push(heading);
        }
        if (lines[i].toLowerCase().indexOf(q) === -1) continue;
        const line = lines[i].replace(/^#+\s*/, '').replace(/[`*_|\\]/g, '').trim();
        if (!line) continue;
        const breadcrumb = [(DOC_TITLES[page] || page), ...headers].join(' > ');
        results.push({ page, line, title: breadcrumb });
        if (results.length >= 20) break;
      }
      if (results.length >= 20) break;
    }
    return results;
  }

  // Initial render
  buildNav();
  renderContent(currentPage);

  return {
    showPage,
    showPageAndScroll,
    search,
    getCurrentPage() {
      return currentPage;
    },
  };
}
