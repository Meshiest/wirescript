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
  let bracketDepth = 0;
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
    if (indent === 0 && parenDepth === 0 && bracketDepth === 0 && result.length > 0) {
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

    // Closing bracket decreases array-literal continuation indent
    const startsWithCloseBracket = trimmed.startsWith("]");
    if (startsWithCloseBracket) {
      bracketDepth = Math.max(0, bracketDepth - 1);
    }

    result.push(tab.repeat(indent + parenDepth + bracketDepth) + trimmed);
    prevTrimmed = trimmed;

    // Count delimiters outside strings. Note `string[] = [` nets +1 bracket:
    // the `[]` type suffix self-cancels, the literal opener carries over.
    const counts = countDelims(trimmed);

    // Braces set block indent
    if (startsWithClose) {
      indent += counts.openBrace;
    } else {
      indent = Math.max(0, indent + counts.openBrace - counts.closeBrace);
    }

    // Track paren depth for continuation indent
    if (startsWithCloseParen) {
      parenDepth += counts.openParen;
    } else {
      parenDepth = Math.max(0, parenDepth + counts.openParen - counts.closeParen);
    }

    // Track bracket depth for multi-line array literals
    if (startsWithCloseBracket) {
      bracketDepth += counts.openBracket;
    } else {
      bracketDepth = Math.max(
        0,
        bracketDepth + counts.openBracket - counts.closeBracket,
      );
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

// Count `{}`/`()`/`[]` occurrences outside string literals in one pass.
// A `//` comment ends the scan so bracket-looking text in comments is ignored.
function countDelims(trimmed) {
  const counts = {
    openBrace: 0,
    closeBrace: 0,
    openParen: 0,
    closeParen: 0,
    openBracket: 0,
    closeBracket: 0,
  };
  let inStr = false,
    strChar = "",
    escaped = false;
  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i];
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
    if (ch === "/" && trimmed[i + 1] === "/") break;
    if (ch === "{") counts.openBrace++;
    if (ch === "}") counts.closeBrace++;
    if (ch === "(") counts.openParen++;
    if (ch === ")") counts.closeParen++;
    if (ch === "[") counts.openBracket++;
    if (ch === "]") counts.closeBracket++;
  }
  return counts;
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
