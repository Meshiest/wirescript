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
 * @returns {monaco.editor.IStandaloneCodeEditor}
 */
export function createEditor(container, wasm, monaco, getFilesJson) {
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
        message: d.message + (d.code ? ` [${d.code}]` : ''),
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
    triggerCharacters: ['.', '('],
    provideCompletionItems(model, position) {
      const source = model.getValue();
      const line = position.lineNumber - 1;
      const col = position.column - 1;
      try {
        const fj = getFilesJson ? getFilesJson() : undefined;
        const json = wasm.wirescript_completions(source, line, col, fj);
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
