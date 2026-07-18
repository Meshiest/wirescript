#!/usr/bin/env node
// Build a ranked search index from docs/*.md for the playground.
// Uses TF-IDF keyword extraction so searching "array push" ranks
// the array methods section above a passing mention of "push".
//
// Output: docs/search-index.json
//
// Each entry: { page, section, text, keywords: {word: score} }

import { readFileSync, writeFileSync } from 'fs';
import { join } from 'path';

const DOCS_DIR = process.argv[2] || 'docs';
const OUT = process.argv[3] || join(DOCS_DIR, 'search-index.json');

// Keys must match DOC_PAGES in docs.js (the page key, not the display title)
const MANIFEST = {
  'README.md': 'Overview',
  'syntax.md': 'Syntax',
  'types.md': 'Types',
  'expressions.md': 'Expressions',
  'statements.md': 'Statements',
  'chips.md': 'Chips & Mods',
  'builtins.md': 'Builtins',
  'exec-context.md': 'Exec Context',
  'best-practices.md': 'Best Practices',
};

// ── Stop words (common English words that add no search value) ──
const STOP = new Set([
  'a',
  'an',
  'the',
  'is',
  'are',
  'was',
  'were',
  'be',
  'been',
  'being',
  'have',
  'has',
  'had',
  'do',
  'does',
  'did',
  'will',
  'would',
  'should',
  'could',
  'may',
  'might',
  'shall',
  'can',
  'to',
  'of',
  'in',
  'for',
  'on',
  'with',
  'at',
  'by',
  'from',
  'as',
  'into',
  'through',
  'during',
  'before',
  'after',
  'above',
  'below',
  'between',
  'out',
  'up',
  'down',
  'about',
  'against',
  'and',
  'but',
  'or',
  'nor',
  'not',
  'no',
  'so',
  'if',
  'then',
  'else',
  'when',
  'while',
  'where',
  'that',
  'this',
  'these',
  'those',
  'it',
  'its',
  'they',
  'them',
  'their',
  'we',
  'our',
  'you',
  'your',
  'he',
  'she',
  'him',
  'her',
  'what',
  'which',
  'who',
  'how',
  'all',
  'each',
  'every',
  'both',
  'few',
  'more',
  'most',
  'other',
  'some',
  'such',
  'only',
  'just',
  'also',
  'than',
  'too',
  'very',
  'same',
  'any',
  'much',
  'many',
  'own',
  'here',
  'there',
  'used',
  'use',
  'uses',
  'using',
  'example',
  'e.g.',
  'i.e.',
  'see',
  'like',
  'one',
  'two',
]);

function tokenize(text) {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9_]/g, ' ')
    .split(/\s+/)
    .filter(w => w.length >= 2 && !STOP.has(w));
}

// ── Parse sections from markdown ──
function parseSections(md, page) {
  const sections = [];
  const lines = md.split('\n');
  const headers = [];
  let currentText = [];
  let currentKind = 'body';
  let inCode = false;

  function flush() {
    const text = currentText.join('\n').trim();
    if (text) {
      sections.push({
        page,
        section: [page, ...headers].join(' > '),
        text,
        kind: currentKind,
      });
    }
    currentText = [];
  }

  for (const line of lines) {
    if (line.startsWith('```')) {
      if (inCode) {
        currentKind = 'code';
        flush();
        inCode = false;
        currentKind = 'body';
      } else {
        flush();
        inCode = true;
      }
      continue;
    }
    if (inCode) {
      currentText.push(line);
      continue;
    }

    const hMatch = line.match(/^(#{1,4})\s+(.+)/);
    if (hMatch) {
      flush();
      const level = hMatch[1].length;
      const heading = hMatch[2].replace(/[`*_|\\]/g, '').trim();
      while (headers.length >= level) headers.pop();
      headers.push(heading);
      sections.push({
        page,
        section: [page, ...headers.slice(0, -1)].join(' > '),
        text: heading,
        kind: 'heading',
      });
      continue;
    }

    // Table row
    if (line.includes('|') && line.split('|').length > 3) {
      const cells = line
        .split('|')
        .map(c => c.replace(/[`*_|\\]/g, '').trim())
        .filter(Boolean);
      const clean = cells.filter(c => !c.match(/^[-:]+$/)).join(' ');
      if (clean) currentText.push(clean);
    } else {
      const clean = line.replace(/[`*_|\\]/g, '').trim();
      if (clean && !clean.match(/^[-:| ]+$/)) currentText.push(clean);
    }
  }
  flush();
  return sections;
}

// ── Build index with TF-IDF ──
const allSections = [];
for (const [file, page] of Object.entries(MANIFEST)) {
  let md;
  try {
    md = readFileSync(join(DOCS_DIR, file), 'utf-8');
  } catch {
    continue;
  }
  allSections.push(...parseSections(md, page));
}

// Document frequency: how many sections contain each word
const df = {};
const N = allSections.length;
for (const sec of allSections) {
  const words = new Set(tokenize(sec.text));
  for (const w of words) {
    df[w] = (df[w] || 0) + 1;
  }
}

// Kind boost: headings are worth more than code, code more than body
const KIND_BOOST = { heading: 4, code: 2, body: 1 };

// Build final entries with keyword scores
const entries = allSections.map(sec => {
  const words = tokenize(sec.text);
  const tf = {};
  for (const w of words) {
    tf[w] = (tf[w] || 0) + 1;
  }
  const keywords = {};
  const boost = KIND_BOOST[sec.kind] || 1;
  for (const [word, count] of Object.entries(tf)) {
    const idf = Math.log(N / (df[word] || 1));
    keywords[word] = Math.round(count * idf * boost * 100) / 100;
  }
  // Truncate text for display
  const display =
    sec.text.length > 120 ? sec.text.substring(0, 120) + '...' : sec.text;
  return {
    p: sec.page,
    s: sec.section,
    t: display,
    k: keywords,
  };
});

writeFileSync(OUT, JSON.stringify(entries));
const sizeKB = (Buffer.byteLength(JSON.stringify(entries)) / 1024).toFixed(1);
console.log(
  `Built search index: ${entries.length} entries, ${sizeKB}KB → ${OUT}`,
);
