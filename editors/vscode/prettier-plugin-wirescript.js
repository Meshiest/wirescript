"use strict";

const languages = [
  {
    name: "Wirescript",
    parsers: ["wirescript"],
    extensions: [".ws"],
    vscodeLanguageIds: ["wirescript"],
  },
];

function parse(text) {
  return { type: "root", body: text };
}
function locStart() {
  return 0;
}
function locEnd(node) {
  return node.body.length;
}

function formatWirescript(source, tabWidth, useTabs) {
  const tab = useTabs ? "\t" : " ".repeat(tabWidth);
  const lines = source.split("\n");
  const result = [];
  // Stack of open delimiters; each entry records whether that open added an
  // indent level. A line adds AT MOST one level no matter how many groups it
  // opens (`addRole(next, {` opens `(` and `{` but indents once).
  const stack = [];
  let prevBlank = false;
  let prevTrimmed = "";

  for (const line of lines) {
    let trimmed = line.trim();

    // Skip empty lines (collapse multiples)
    if (trimmed === "") {
      if (!prevBlank && result.length > 0) {
        result.push("");
        prevBlank = true;
      }
      continue;
    }
    prevBlank = false;

    // --- Opinionated formatting ---

    // Normalize spaces around = (assignment), but not ==, !=, <=, >=
    // Process outside of strings
    trimmed = formatSpacing(trimmed);

    // Remove blank line between consecutive same-kind top-level declarations
    if (stack.length === 0 && result.length > 0) {
      const lastNonBlank = prevTrimmed;
      const shouldCollapse =
        // Same declaration kind
        (startsWithSameKw(trimmed, lastNonBlank, "in ") ||
          startsWithSameKw(trimmed, lastNonBlank, "out ") ||
          startsWithSameKw(trimmed, lastNonBlank, "var ")) &&
        result[result.length - 1] === "";
      if (shouldCollapse) {
        result.pop(); // remove the blank line
      }
    }

    // A leading run of closers de-indents before printing so the closing
    // line sits at its opener's level (`}`, `)`, `]`, `})`, ...).
    const code = stripLineComment(trimmed);
    let leadingClosers = 0;
    while (
      leadingClosers < code.length &&
      (code[leadingClosers] === "}" ||
        code[leadingClosers] === ")" ||
        code[leadingClosers] === "]")
    ) {
      leadingClosers++;
    }
    for (let i = 0; i < leadingClosers; i++) {
      stack.pop();
    }

    const lineIndent = stack.filter(Boolean).length;
    result.push(tab.repeat(lineIndent) + trimmed);
    prevTrimmed = trimmed;

    // Scan the rest of the line: opens push (adding an indent level only
    // while this line has no net open level yet), closes pop.
    scanDelims(code, leadingClosers, stack);
  }

  // Remove trailing blank lines
  while (result.length > 0 && result[result.length - 1] === "") {
    result.pop();
  }

  return result.join("\n") + "\n";
}

// Push/pop `{}`/`()`/`[]` found outside string literals onto the depth
// stack, skipping the first `leading` characters (already popped by the
// caller). An open adds an indent contribution only while the line's net
// indenting opens are zero — at most one level per line.
function scanDelims(code, leading, stack) {
  let inStr = false,
    strChar = "",
    escaped = false;
  let netTrue = 0;
  for (let i = leading; i < code.length; i++) {
    const ch = code[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (!inStr && (ch === '"' || ch === "'")) {
      inStr = true;
      strChar = ch;
      continue;
    }
    if (inStr && ch === strChar) {
      inStr = false;
      continue;
    }
    if (inStr) continue;
    if (ch === "{" || ch === "(" || ch === "[") {
      const adds = netTrue <= 0;
      if (adds) netTrue++;
      stack.push(adds);
    } else if (ch === "}" || ch === ")" || ch === "]") {
      if (stack.length > 0 && stack.pop()) netTrue--;
    }
  }
}

// Slice off a `//` comment (outside strings) so bracket-looking text in
// comments doesn't skew indentation.
function stripLineComment(line) {
  let inStr = false,
    strChar = "",
    escaped = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (!inStr && (ch === '"' || ch === "'")) {
      inStr = true;
      strChar = ch;
      continue;
    }
    if (inStr && ch === strChar) {
      inStr = false;
      continue;
    }
    if (inStr) continue;
    if (ch === "/" && line[i + 1] === "/") return line.slice(0, i);
  }
  return line;
}

// Normalize spacing around operators, respecting strings
function formatSpacing(line) {
  let out = "";
  let inStr = false;
  let strChar = "";
  let escaped = false;

  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    const next = line[i + 1] || "";
    const prev = line[i - 1] || "";

    if (escaped) {
      out += ch;
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      out += ch;
      escaped = true;
      continue;
    }
    if (!inStr && (ch === '"' || ch === "'")) {
      inStr = true;
      strChar = ch;
      out += ch;
      continue;
    }
    if (inStr && ch === strChar) {
      inStr = false;
      out += ch;
      continue;
    }
    if (inStr) {
      out += ch;
      continue;
    }

    // Collapse multiple spaces to one (outside strings)
    if (ch === " " && next === " " && prev !== " ") {
      // Keep single space
      out += ch;
      // Skip remaining spaces
      while (i + 1 < line.length && line[i + 1] === " ") i++;
      continue;
    }

    out += ch;
  }

  return out;
}

function startsWithSameKw(a, b, kw) {
  return a.startsWith(kw) && b.startsWith(kw);
}

const printers = {
  "wirescript-ast": {
    print(path, options) {
      const node = path.getValue();
      return formatWirescript(
        node.body,
        options.tabWidth || 2,
        options.useTabs || false,
      );
    },
  },
};

const parsers = {
  wirescript: {
    parse,
    astFormat: "wirescript-ast",
    locStart,
    locEnd,
  },
};

module.exports = { languages, parsers, printers };
