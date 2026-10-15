import { test } from 'node:test';
import assert from 'node:assert/strict';
import { blockRules, collectLibraries, importsOf, isCompilable, preludeBefore, wordsFromClasses } from '../snippet.js';

test('fence words come back from the rendered classes', () => {
  const words = wordsFromClasses(['language-wirescript', 'name=door', 'hljs']);
  assert.deepEqual(words, ['name=door']);
  assert.equal(isCompilable(blockRules(words)), true);
  assert.equal(isCompilable(blockRules(['ignore'])), false);
  assert.equal(isCompilable(blockRules(['name=bad', 'expect=WSP001'])), false);
  assert.equal(isCompilable(blockRules(['nocompile', 'name=x'])), false);
  // A block without a name is never offered as a download.
  assert.equal(isCompilable(blockRules([])), false);
});

test('finds the modules a source imports', () => {
  const src = 'import { escape } from "sanitize"\nimport * as u from \'utils\'\nlet x = 1 // import "not-this"\n';
  assert.deepEqual(importsOf(src), ['sanitize', 'utils']);
  assert.deepEqual(importsOf('import {\n  escape,\n  sanitize,\n} from "sanitize"\n'), ['sanitize']);
});

test('collects libraries and the libraries they import, once each', async () => {
  const libs = {
    sanitize: 'import { trimAll } from "text"\nmod escape(t: string) -> string { return t }',
    text: 'mod trimAll(t: string) -> string { return t }',
  };
  const fetched = [];
  const files = await collectLibraries('import { escape } from "sanitize"\nimport { trimAll } from "text"', async (name) => {
    fetched.push(name);
    return libs[name];
  });
  assert.deepEqual(Object.keys(files).sort(), ['sanitize.ws', 'text.ws']);
  assert.deepEqual(fetched, ['sanitize', 'text']);
});

/** A minimal DOM: siblings linked through `previousSibling`. */
function siblings(...nodes) {
  for (let i = 0; i < nodes.length; i++) nodes[i].previousSibling = nodes[i - 1] || null;
  return nodes[nodes.length - 1];
}
const comment = (text) => ({ nodeType: 8, textContent: text });
const space = () => ({ nodeType: 3, textContent: '\n\n' });
const element = () => ({ nodeType: 1, textContent: 'paragraph' });

test('reads the hidden preludes before a block, earliest first', () => {
  const pre = siblings(
    element(),
    comment(' doc-check-prelude:\nin go: exec\n'),
    space(),
    comment(' doc-check-prelude:\nvar n: int = 0\n'),
    space(),
    { nodeType: 1, textContent: 'the block' },
  );
  assert.equal(preludeBefore(pre), 'in go: exec\nvar n: int = 0\n');
});

test('a paragraph between ends the prelude, and an ordinary comment is not one', () => {
  const pre = siblings(comment(' doc-check-prelude:\nin go: exec\n'), element(), { nodeType: 1 });
  assert.equal(preludeBefore(pre), '');
  const other = siblings(comment(' toc '), { nodeType: 1 });
  assert.equal(preludeBefore(other), '');
});
