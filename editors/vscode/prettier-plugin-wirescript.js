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
  let indent = 0;
  let parenDepth = 0;
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
    if (indent === 0 && parenDepth === 0 && result.length > 0) {
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

    // Closing brace decreases indent
    const startsWithClose = trimmed.startsWith("}");
    if (startsWithClose) {
      indent = Math.max(0, indent - 1);
    }

    // Closing paren decreases paren continuation indent
    const startsWithCloseParen = trimmed.startsWith(")");
    if (startsWithCloseParen) {
      parenDepth = Math.max(0, parenDepth - 1);
    }

    result.push(tab.repeat(indent + parenDepth) + trimmed);
    prevTrimmed = trimmed;

    // Count braces outside strings
    const delta = countBraceDelta(trimmed);
    if (startsWithClose) {
      indent += countOpens(trimmed);
    } else {
      indent = Math.max(0, indent + delta);
    }

    // Track paren depth for continuation indent
    const parenDelta = countParenDelta(trimmed);
    if (startsWithCloseParen) {
      parenDepth += countOpenParens(trimmed);
    } else {
      parenDepth = Math.max(0, parenDepth + parenDelta);
    }
  }

  // Remove trailing blank lines
  while (result.length > 0 && result[result.length - 1] === "") {
    result.pop();
  }

  return result.join("\n") + "\n";
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

function countBraceDelta(trimmed) {
  let opens = 0,
    closes = 0;
  let inStr = false,
    strChar = "",
    escaped = false;
  for (const ch of trimmed) {
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
    if (ch === "{") opens++;
    if (ch === "}") closes++;
  }
  return opens - closes;
}

function countOpens(trimmed) {
  let opens = 0;
  let inStr = false,
    strChar = "",
    escaped = false;
  for (const ch of trimmed) {
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
    if (ch === "{") opens++;
  }
  return opens;
}

function countParenDelta(trimmed) {
  let opens = 0,
    closes = 0;
  let inStr = false,
    strChar = "",
    escaped = false;
  for (const ch of trimmed) {
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
    if (ch === "(") opens++;
    if (ch === ")") closes++;
  }
  return opens - closes;
}

function countOpenParens(trimmed) {
  let opens = 0;
  let inStr = false,
    strChar = "",
    escaped = false;
  for (const ch of trimmed) {
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
    if (ch === "(") opens++;
  }
  return opens;
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
