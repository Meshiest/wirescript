// editor.js -- Monaco editor setup with Wirescript language services

import { monarchLanguage, monarchConfiguration } from './monarch.js';

const LANGUAGE_ID = 'wirescript';

// Map completion kind strings from the WASM API to Monaco CompletionItemKind values
function mapCompletionKind(monaco, kind) {
  const map = {
    keyword: monaco.languages.CompletionItemKind.Keyword,
    variable: monaco.languages.CompletionItemKind.Variable,
    function: monaco.languages.CompletionItemKind.Function,
    type: monaco.languages.CompletionItemKind.Class,
    field: monaco.languages.CompletionItemKind.Field,
    property: monaco.languages.CompletionItemKind.Property,
    constant: monaco.languages.CompletionItemKind.Constant,
    event: monaco.languages.CompletionItemKind.Event,
    module: monaco.languages.CompletionItemKind.Module,
    snippet: monaco.languages.CompletionItemKind.Snippet,
    value: monaco.languages.CompletionItemKind.Value,
    enum: monaco.languages.CompletionItemKind.Enum,
    operator: monaco.languages.CompletionItemKind.Operator,
    unit: monaco.languages.CompletionItemKind.Unit,
    text: monaco.languages.CompletionItemKind.Text,
    method: monaco.languages.CompletionItemKind.Method,
    constructor: monaco.languages.CompletionItemKind.Constructor,
    interface: monaco.languages.CompletionItemKind.Interface,
    struct: monaco.languages.CompletionItemKind.Struct,
    reference: monaco.languages.CompletionItemKind.Reference,
  };
  return map[kind] || monaco.languages.CompletionItemKind.Text;
}

// Map diagnostic severity strings to Monaco MarkerSeverity
function mapSeverity(monaco, severity) {
  const s = (severity || '').toLowerCase();
  if (s === 'error') return monaco.MarkerSeverity.Error;
  if (s === 'warning') return monaco.MarkerSeverity.Warning;
  if (s === 'info') return monaco.MarkerSeverity.Info;
  if (s === 'hint') return monaco.MarkerSeverity.Hint;
  return monaco.MarkerSeverity.Error;
}

/**
 * Register the wirescript language and all providers, create and return an editor instance.
 *
 * @param {HTMLElement} container - DOM element to host the editor
 * @param {object} wasm - The WASM module exports (wirescript_compile, wirescript_diagnostics, etc.)
 * @param {object} monaco - The Monaco editor module
 * @param {function} [getFilesJson] - Returns the imports map JSON (`{ "utils.ws": "<source>" }`).
 * @param {function} [getPrefabsJson] - Returns the dragged-in prefab registry JSON
 *   (`{ "./turret.brz": [<byte>, ...] }`) — bytes as a number array. Drives
 *   `$./file.brz` completion and embedding at compile.
 * @returns {monaco.editor.IStandaloneCodeEditor}
 */
export function createEditor(container, wasm, monaco, getFilesJson, getPrefabsJson) {
  // Register the language
  monaco.languages.register({ id: LANGUAGE_ID });
  monaco.languages.setMonarchTokensProvider(LANGUAGE_ID, monarchLanguage);
  monaco.languages.setLanguageConfiguration(LANGUAGE_ID, monarchConfiguration);

  // Define the theme
  monaco.editor.defineTheme('wirescript-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '6A9955', fontStyle: 'italic' },
      { token: 'comment.doc', foreground: '608B4E', fontStyle: 'italic' },
      { token: 'keyword', foreground: 'C586C0' },
      { token: 'type', foreground: '4EC9B0' },
      { token: 'annotation', foreground: 'D7BA7D' },
      { token: 'support.function', foreground: 'DCDCAA' },
      { token: 'variable.predefined', foreground: '4FC1FF' },
      { token: 'constant', foreground: '569CD6' },
      { token: 'number', foreground: 'B5CEA8' },
      { token: 'number.hex', foreground: 'B5CEA8' },
      { token: 'number.binary', foreground: 'B5CEA8' },
      { token: 'number.float', foreground: 'B5CEA8' },
      { token: 'string', foreground: 'CE9178' },
      { token: 'string.escape', foreground: 'D7BA7D' },
      { token: 'string.escape.invalid', foreground: 'F44747' },
      { token: 'operator', foreground: 'D4D4D4' },
      { token: 'delimiter', foreground: 'D4D4D4' },
      { token: 'delimiter.bracket', foreground: 'FFD700' },
      { token: 'identifier', foreground: '9CDCFE' },
    ],
    colors: {
      'editor.background': '#1e1e1e',
      'editor.foreground': '#d4d4d4',
    },
  });

  // Create the editor
  const editor = monaco.editor.create(container, {
    language: LANGUAGE_ID,
    theme: 'wirescript-dark',
    automaticLayout: true,
    minimap: { enabled: false },
    fontSize: 14,
    lineNumbers: 'on',
    renderWhitespace: 'selection',
    scrollBeyondLastLine: false,
    inlayHints: { enabled: 'offUnlessPressed' },
    tabSize: 2,
    insertSpaces: true,
    wordWrap: 'off',
    bracketPairColorization: { enabled: true },
    padding: { top: 8 },
  });

  // -- Diagnostics with debounce --
  let diagnosticTimer = null;

  function updateDiagnostics() {
    const model = editor.getModel();
    if (!model) return;
    const source = model.getValue();
    try {
      const fj = getFilesJson ? getFilesJson() : undefined;
      const json = wasm.wirescript_diagnostics(source, fj);
      const diagnostics = JSON.parse(json);
      const markers = diagnostics.map(d => ({
        severity: mapSeverity(monaco, d.severity),
        message: d.message,
        // The code shows beside the message, linked to its docs section.
        code: d.docUrl ? { value: d.code, target: monaco.Uri.parse(d.docUrl) } : d.code || undefined,
        startLineNumber: (d.startLine || 0) + 1,
        startColumn: (d.startCol || 0) + 1,
        endLineNumber: (d.endLine || d.startLine || 0) + 1,
        endColumn: (d.endCol || d.startCol || 0) + 2,
      }));
      monaco.editor.setModelMarkers(model, LANGUAGE_ID, markers);
    } catch (e) {
      // Clear markers on error
      monaco.editor.setModelMarkers(model, LANGUAGE_ID, []);
    }
  }

  function scheduleDiagnostics() {
    clearTimeout(diagnosticTimer);
    diagnosticTimer = setTimeout(updateDiagnostics, 500);
  }

  editor.onDidChangeModelContent(scheduleDiagnostics);
  // Run diagnostics initially
  setTimeout(updateDiagnostics, 100);

  // -- Completion Provider --
  monaco.languages.registerCompletionItemProvider(LANGUAGE_ID, {
    triggerCharacters: ['.', '(', '$', '/'],
    provideCompletionItems(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const fj = getFilesJson ? getFilesJson() : undefined;
        const pj = getPrefabsJson ? getPrefabsJson() : undefined;
        const json = wasm.wirescript_completions(source, line, col, fj, pj);
        if (!json) return { suggestions: [] };
        const items = JSON.parse(json);
        const word = model.getWordUntilPosition(position);
        const range = {
          startLineNumber: position.lineNumber,
          endLineNumber: position.lineNumber,
          startColumn: word.startColumn,
          endColumn: word.endColumn,
        };
        const suggestions = items.map(item => {
          const isFunc = item.kind === 'function' || item.kind === 'method';
          const isMod = item.kind === 'mod' || item.kind === 'chip';
          const needsParens = (isFunc || isMod) && !item.insertText;
          let insertText = item.insertText || item.label;
          let insertTextRules;
          if (needsParens) {
            insertText = item.label + '($1)';
            insertTextRules = monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet;
          } else if (insertText.includes('$')) {
            insertTextRules = monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet;
          }
          return {
            label: item.label,
            kind: mapCompletionKind(monaco, item.kind),
            detail: item.detail || '',
            insertText,
            insertTextRules,
            range,
          };
        });
        return { suggestions };
      } catch (e) {
        return { suggestions: [] };
      }
    },
  });

  // -- Hover Provider --
  monaco.languages.registerHoverProvider(LANGUAGE_ID, {
    provideHover(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const fj = getFilesJson ? getFilesJson() : undefined;
        const result = wasm.wirescript_hover(source, line, col, fj);
        if (!result) return null;
        try {
          const parsed = JSON.parse(result);
          return {
            contents: [{ value: parsed.value }],
          };
        } catch {
          return {
            contents: [{ value: result }],
          };
        }
      } catch (e) {
        return null;
      }
    },
  });

  // -- Definition Provider --
  monaco.languages.registerDefinitionProvider(LANGUAGE_ID, {
    provideDefinition(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const json = wasm.wirescript_definition(source, line, col);
        if (!json) return null;
        const def = JSON.parse(json);
        if (!def || def.startLine === undefined) return null;
        return {
          uri: model.uri,
          range: {
            startLineNumber: def.startLine + 1,
            startColumn: def.startCol + 1,
            endLineNumber: def.endLine + 1,
            endColumn: def.endCol + 1,
          },
        };
      } catch (e) {
        return null;
      }
    },
  });

  // -- Reference Provider --
  monaco.languages.registerReferenceProvider(LANGUAGE_ID, {
    provideReferences(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const json = wasm.wirescript_references(source, line, col);
        if (!json) return [];
        const refs = JSON.parse(json);
        if (!Array.isArray(refs)) return [];
        return refs.map(r => ({
          uri: model.uri,
          range: {
            startLineNumber: r.startLine + 1,
            startColumn: r.startCol + 1,
            endLineNumber: r.endLine + 1,
            endColumn: r.endCol + 1,
          },
        }));
      } catch (e) {
        return [];
      }
    },
  });

  // -- Rename Provider --
  monaco.languages.registerRenameProvider(LANGUAGE_ID, {
    provideRenameEdits(model, position, newName) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const json = wasm.wirescript_references(source, line, col);
        if (!json) return null;
        const refs = JSON.parse(json);
        if (!Array.isArray(refs) || refs.length === 0) return null;
        return {
          edits: refs.map(r => ({
            resource: model.uri,
            textEdit: {
              range: {
                startLineNumber: r.startLine + 1,
                startColumn: r.startCol + 1,
                endLineNumber: r.endLine + 1,
                endColumn: r.endCol + 1,
              },
              text: newName,
            },
            versionId: undefined,
          })),
        };
      } catch (e) {
        return null;
      }
    },
    resolveRenameLocation(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      const lineText = model.getLineContent(position.lineNumber);
      const c = Math.min(col, lineText.length);
      const before = lineText.substring(0, c);
      const after = lineText.substring(c);
      const startMatch = before.match(/[a-zA-Z_]\w*$/);
      const endMatch = after.match(/^\w*/);
      if (!startMatch) return { rejectReason: 'Not a renameable symbol' };
      const wordStart = c - startMatch[0].length;
      const wordEnd = c + (endMatch ? endMatch[0].length : 0);
      const word = lineText.substring(wordStart, wordEnd);
      if (!word) return { rejectReason: 'Not a renameable symbol' };
      return {
        range: {
          startLineNumber: position.lineNumber,
          startColumn: wordStart + 1,
          endLineNumber: position.lineNumber,
          endColumn: wordEnd + 1,
        },
        text: word,
      };
    },
  });

  // -- Document Formatting Provider --
  monaco.languages.registerDocumentFormattingEditProvider(LANGUAGE_ID, {
    provideDocumentFormattingEdits(model, options) {
      const source = model.getValue();
      try {
        const formatted = wasm.wirescript_format(
          source,
          options.tabSize,
          !options.insertSpaces
        );
        if (formatted === source) return [];
        const fullRange = model.getFullModelRange();
        return [
          {
            range: fullRange,
            text: formatted,
          },
        ];
      } catch (e) {
        return [];
      }
    },
  });

  // -- Inlay Hints Provider --
  if (wasm.wirescript_inlay_hints) {
    monaco.languages.registerInlayHintsProvider(LANGUAGE_ID, {
      provideInlayHints(model, range) {
        const source = model.getValue();
        try {
          const fj = getFilesJson ? getFilesJson() : undefined;
          const json = wasm.wirescript_inlay_hints(source, fj);
          if (!json) return { hints: [], dispose() {} };
          const items = JSON.parse(json);
          const hints = items.map(h => ({
            position: { lineNumber: h.line + 1, column: h.col + 1 },
            label: h.label,
            kind: h.kind === 'type'
              ? monaco.languages.InlayHintKind.Type
              : monaco.languages.InlayHintKind.Parameter,
            paddingLeft: h.kind === 'type',
          }));
          return { hints, dispose() {} };
        } catch (e) {
          return { hints: [], dispose() {} };
        }
      },
    });
  }

  return editor;
}

// -- Tests --

/**
 * The code lenses for a file's tests: "Run all tests" above the first line,
 * then "Run test" above each `@test`, its title carrying the last verdict.
 *
 * @param {{name: string, startLine: number}[]} tests - From `wirescript_list_tests`.
 * @param {Map<string, boolean>} [passed] - Each test's last verdict, by name.
 * @returns {{line: number, title: string, names: string[]}[]} Lines 0-based.
 */
export function testLenses(tests, passed = new Map()) {
  if (tests.length === 0) return [];
  const lenses = [{ line: 0, title: `Run all tests (${tests.length})`, names: tests.map(t => t.name) }];
  for (const t of tests) {
    const verdict = passed.get(t.name);
    const title = verdict === undefined ? 'Run test' : verdict ? 'Run test (passed)' : 'Run test (failed)';
    lenses.push({ line: t.startLine, title, names: [t.name] });
  }
  return lenses;
}

/**
 * The simulator panel's rows for test verdicts: per test a PASS or FAIL row,
 * its chat lines (a FAIL line marked as one), then any problem the chat did
 * not show, such as an error or a test that never reached its end.
 *
 * @param {{name: string, passed: boolean, problems: string[], lines: {tick: number, text: string}[]}[]} reports
 * @returns {{kind: 'pass'|'fail'|'line'|'failLine'|'problem', text: string, tick?: number}[]}
 */
export function testReportRows(reports) {
  const rows = [];
  for (const r of reports) {
    rows.push({ kind: r.passed ? 'pass' : 'fail', text: (r.passed ? 'PASS ' : 'FAIL ') + r.name });
    const printed = new Set(r.lines.map(l => l.text));
    for (const l of r.lines) {
      rows.push({ kind: /^\s*FAIL\b/m.test(l.text) ? 'failLine' : 'line', tick: l.tick, text: l.text });
    }
    for (const p of r.problems) {
      if (!printed.has(p)) rows.push({ kind: 'problem', text: p });
    }
  }
  return rows;
}

/**
 * "Run test" lenses above each `@test` and a "Run all tests" lens at the top
 * of a file that has tests; a test that failed its last run gets its line
 * marked, with its problems on hover.
 *
 * @param {object} monaco - The Monaco editor module.
 * @param {monaco.editor.IStandaloneCodeEditor} editor
 * @param {object} wasm - The WASM module exports.
 * @param {function} getModuleName - The active file's module name.
 * @param {function} runTests - Called with the names of the tests to run.
 * @returns {{show: function}} `show(moduleName, reports)` records verdicts.
 */
export function registerTests(monaco, editor, wasm, getModuleName, runTests) {
  // Verdicts by module, then by test name.
  const verdicts = new Map();
  const changed = new monaco.Emitter();
  const marks = editor.createDecorationsCollection();

  // A test's range starts at its keyword; its lens goes above any
  // annotation lines (`@test` on its own line) written over it.
  const listTests = () => {
    const model = editor.getModel();
    try {
      return JSON.parse(wasm.wirescript_list_tests(model.getValue(), getModuleName())).map(t => {
        let line = t.startLine;
        while (line > 0 && model.getLineContent(line).trimStart().startsWith('@')) line--;
        return { ...t, startLine: line };
      });
    } catch (e) {
      return [];
    }
  };
  const handler = (_accessor, names) => runTests(names);
  const commandId = monaco.editor.registerCommand
    ? (monaco.editor.registerCommand('wirescript.runTests', handler), 'wirescript.runTests')
    : editor.addCommand(0, handler, '');

  function mark() {
    const results = verdicts.get(getModuleName()) || new Map();
    marks.set(listTests().flatMap(t => {
      const r = results.get(t.name);
      if (!r || r.passed) return [];
      return [{
        range: new monaco.Range(t.startLine + 1, 1, t.startLine + 1, 1),
        options: {
          isWholeLine: true,
          className: 'wirescript-test-failed',
          linesDecorationsClassName: 'wirescript-test-failed-gutter',
          hoverMessage: { value: '**Test failed**\n\n```text\n' + r.problems.join('\n') + '\n```' },
        },
      }];
    }));
  }

  monaco.languages.registerCodeLensProvider(LANGUAGE_ID, {
    onDidChange: changed.event,
    provideCodeLenses(model) {
      if (model !== editor.getModel()) return { lenses: [], dispose() {} };
      const results = verdicts.get(getModuleName()) || new Map();
      const passed = new Map([...results].map(([name, r]) => [name, r.passed]));
      const lenses = testLenses(listTests(), passed).map(l => ({
        range: new monaco.Range(l.line + 1, 1, l.line + 1, 1),
        command: { id: commandId, title: l.title, arguments: [l.names] },
      }));
      return { lenses, dispose() {} };
    },
  });

  // Switching files replaces the whole text: mark the new file's tests.
  editor.onDidChangeModelContent(e => {
    if (e.isFlush) mark();
  });

  return {
    show(moduleName, reports) {
      const results = verdicts.get(moduleName) || new Map();
      for (const r of reports) results.set(r.name, r);
      verdicts.set(moduleName, results);
      mark();
      changed.fire();
    },
  };
}
