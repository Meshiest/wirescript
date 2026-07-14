"use strict";

// Print width past which a too-long import / binary-operation line is broken.
const PRINT_WIDTH = 100;

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
  // Set by a standalone `// fmt-ignore`; preserves the next line verbatim.
  let ignoreNext = false;

  for (const rawLine of lines) {
    const rawTrimmed = rawLine.trim();

    // Skip empty lines (collapse multiples)
    if (rawTrimmed === "") {
      if (!prevBlank && result.length > 0) {
        result.push("");
        prevBlank = true;
      }
      continue;
    }
    prevBlank = false;

    // --- `// fmt-ignore` escape hatch (like Prettier's `// prettier-ignore`) ---
    // Standalone → preserve the NEXT line verbatim; trailing on a line of code →
    // preserve THAT line. A preserved line is emitted exactly as written (no
    // spacing/indent/split) but still updates the delimiter stack.
    const rawCode = stripLineComment(rawTrimmed);
    const rawComment = rawTrimmed.slice(rawCode.length);
    const isMarker =
      rawComment.startsWith("//") &&
      !rawComment.startsWith("///") &&
      rawComment.replace(/^\/+/, "").trim() === "fmt-ignore";
    const standaloneMarker = isMarker && rawCode.trim() === "";
    const trailingMarker = isMarker && rawCode.trim() !== "";
    const wasIgnore = ignoreNext;
    ignoreNext = false;

    if (wasIgnore || trailingMarker) {
      let leadingClosers = countLeadingClosers(rawCode);
      for (let i = 0; i < leadingClosers; i++) stack.pop();
      result.push(rawLine); // original, untouched
      prevTrimmed = rawTrimmed;
      scanDelims(rawCode, leadingClosers, stack);
      continue;
    }

    // --- Opinionated intra-line spacing (space after commas, etc.) ---
    let trimmed = formatSpacing(rawTrimmed);

    // Join a statement-form `else` onto the previous closing brace:
    // `}\n  else {` → `} else {`. The expression form (`if c then x` then
    // `else y`) has no preceding `}`, so it stays on its own line.
    if (
      /^else(\s|\{|$)/.test(trimmed) &&
      result.length > 0 &&
      result[result.length - 1].trimEnd().endsWith("}")
    ) {
      result[result.length - 1] =
        result[result.length - 1].trimEnd() + " " + trimmed;
      prevTrimmed = trimmed;
      scanDelims(stripLineComment(trimmed), 0, stack);
      continue;
    }

    // Remove blank line between consecutive same-kind top-level declarations
    if (stack.length === 0 && result.length > 0) {
      const lastNonBlank = prevTrimmed;
      const shouldCollapse =
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
    const leadingClosers = countLeadingClosers(code);
    for (let i = 0; i < leadingClosers; i++) {
      stack.pop();
    }
    const startsClose = leadingClosers > 0;

    // `else` on its own line and a line starting with a binary operator are
    // continuations of the previous expression — indent one extra level.
    const isExprElse =
      !startsClose &&
      (trimmed === "else" ||
        trimmed.startsWith("else ") ||
        trimmed.startsWith("else\t"));
    const isBinopCont = !startsClose && isBinopContinuation(trimmed);
    const extra = isExprElse || isBinopCont ? 1 : 0;
    const base = stack.filter(Boolean).length + extra;

    // A standalone `// fmt-ignore` is re-indented normally, then protects the
    // next line.
    if (standaloneMarker) {
      result.push(tab.repeat(base) + trimmed);
      prevTrimmed = trimmed;
      ignoreNext = true;
      scanDelims(code, leadingClosers, stack);
      continue;
    }

    // Split over-long primary statement lines (imports / binary operations).
    const single = tab.repeat(base) + trimmed;
    const splittable = extra === 0 && !startsClose && code.length === trimmed.length;
    let outLines = [single];
    if (splittable && displayWidth(single) > PRINT_WIDTH) {
      outLines =
        splitImport(trimmed, base, tab) ||
        splitOperation(trimmed, code, base, tab) ||
        [single];
    }
    for (const ol of outLines) result.push(ol);
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

function displayWidth(s) {
  return s.length;
}

function countLeadingClosers(code) {
  let n = 0;
  while (n < code.length && "})]".includes(code[n])) n++;
  return n;
}

const CONTINUATION_OPS = [
  "&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", "..",
  "&", "|", "^", "+", "-", "*", "/", "%", "<", ">",
];

// A line starting with a binary operator is a continuation of the line above.
function isBinopContinuation(trimmed) {
  return CONTINUATION_OPS.some((op) => {
    if (!trimmed.startsWith(op)) return false;
    const rest = trimmed.slice(op.length);
    return rest.length > 0 && (/^\s/.test(rest) || rest[0] === "(");
  });
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

// Replace string-literal contents (and the quotes) with spaces so a following
// structural scan ignores brackets/operators/braces that live inside strings.
function blankStrings(code) {
  let out = "";
  let inStr = false,
    strChar = "",
    escaped = false;
  for (const ch of code) {
    if (escaped) {
      out += " ";
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      out += " ";
      escaped = true;
      continue;
    }
    if (inStr) {
      if (ch === strChar) inStr = false;
      out += " ";
      continue;
    }
    if (ch === '"' || ch === "'") {
      inStr = true;
      strChar = ch;
      out += " ";
      continue;
    }
    out += ch;
  }
  return out;
}

// Normalize intra-line spacing outside strings and comments: collapse runs of
// spaces, drop any space before a comma, and put exactly one space after a
// comma (unless a closer / end-of-line follows).
function formatSpacing(line) {
  let out = "";
  let inStr = false;
  let strChar = "";
  let escaped = false;

  for (let i = 0; i < line.length; i++) {
    const ch = line[i];

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

    // A `//` comment: copy the rest of the line verbatim.
    if (ch === "/" && line[i + 1] === "/") {
      out += line.slice(i);
      break;
    }

    // Comma: no space before, exactly one space after.
    if (ch === ",") {
      out = out.replace(/[ \t]+$/, "");
      out += ",";
      let j = i + 1;
      while (j < line.length && (line[j] === " " || line[j] === "\t")) j++;
      const next = line[j];
      if (next !== undefined && next !== ")" && next !== "]" && next !== "}") {
        out += " ";
      }
      i = j - 1;
      continue;
    }

    // Collapse multiple spaces to one (outside strings)
    if (ch === " " && line[i + 1] === " ") {
      out += " ";
      while (i + 1 < line.length && line[i + 1] === " ") i++;
      continue;
    }

    out += ch;
  }

  return out;
}

// Precedence of a splittable binary operator (lower binds looser → break there
// first). `null` for non-binary / assignment operators.
function binopPrec(op) {
  switch (op) {
    case "||":
    case "^^":
      return 1;
    case "&&":
      return 2;
    case "|":
      return 3;
    case "^":
      return 4;
    case "&":
      return 5;
    case "==":
    case "!=":
      return 6;
    case "<":
    case ">":
    case "<=":
    case ">=":
      return 7;
    case "<<":
    case ">>":
      return 8;
    case "..":
      return 9;
    case "+":
    case "-":
      return 10;
    case "*":
    case "/":
    case "%":
      return 11;
    case "**":
      return 12;
    default:
      return null;
  }
}

// Positions of depth-0 binary operators in `code` (comment already stripped),
// as `{ pos, op, prec }`. Handles strings, unary `+`/`-`, `...` spread, `->`.
function topLevelBinops(code) {
  const ops = [];
  let inStr = false,
    strChar = "",
    escaped = false,
    depth = 0,
    prevSig = "";
  const isIdent = (c) =>
    (c >= "0" && c <= "9") ||
    (c >= "A" && c <= "Z") ||
    (c >= "a" && c <= "z") ||
    c === "_";
  const endsOperand = (c) =>
    isIdent(c) || c === ")" || c === "]" || c === '"' || c === "'";
  let i = 0;
  const n = code.length;
  while (i < n) {
    const ch = code[i];
    if (escaped) {
      escaped = false;
      i++;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      i++;
      continue;
    }
    if (inStr) {
      if (ch === strChar) {
        inStr = false;
        prevSig = ch;
      }
      i++;
      continue;
    }
    if (ch === '"' || ch === "'") {
      inStr = true;
      strChar = ch;
      i++;
      continue;
    }
    if (ch === " " || ch === "\t") {
      i++;
      continue;
    }
    if (ch === "{" || ch === "(" || ch === "[") {
      depth++;
      prevSig = ch;
      i++;
      continue;
    }
    if (ch === "}" || ch === ")" || ch === "]") {
      depth--;
      prevSig = ch;
      i++;
      continue;
    }
    const two = code.substr(i, 2);
    if (code.substr(i, 3) === "...") {
      prevSig = ".";
      i += 3;
      continue;
    }
    if (two === "->" || two === "=>") {
      prevSig = two[1];
      i += 2;
      continue;
    }
    if (
      ["&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", ".."].includes(
        two,
      )
    ) {
      if ((two === "<<" || two === ">>") && code[i + 2] === "=") {
        prevSig = "=";
        i += 3;
        continue;
      }
      if (depth === 0) {
        const p = binopPrec(two);
        if (p !== null) ops.push({ pos: i, op: two, prec: p });
      }
      prevSig = two[1];
      i += 2;
      continue;
    }
    if ("+-*/%&|^<>".includes(ch)) {
      if (code[i + 1] === "=") {
        prevSig = "=";
        i += 2;
        continue; // compound assignment (+=, <<= handled above, etc.)
      }
      if ((ch === "+" || ch === "-") && !endsOperand(prevSig)) {
        prevSig = ch;
        i++;
        continue; // unary sign
      }
      if (depth === 0) {
        const p = binopPrec(ch);
        if (p !== null) ops.push({ pos: i, op: ch, prec: p });
      }
      prevSig = ch;
      i++;
      continue;
    }
    prevSig = ch;
    i++;
  }
  return ops;
}

function isBalanced(code) {
  const bare = blankStrings(code);
  let d = 0;
  for (const ch of bare) {
    if (ch === "(" || ch === "{" || ch === "[") d++;
    else if (ch === ")" || ch === "}" || ch === "]") {
      d--;
      if (d < 0) return false;
    }
  }
  return d === 0;
}

// Split `import { a, b, c } from "…"` whose name list overflows, filling as many
// names per continuation line as fit. `null` when not a braced import.
function splitImport(trimmed, base, tab) {
  if (!/^import\b/.test(trimmed)) return null;
  const bare = blankStrings(trimmed);
  const lb = bare.indexOf("{");
  if (lb < 0) return null;
  const rb = bare.indexOf("}", lb + 1);
  if (rb < 0) return null;
  const prefix = trimmed.slice(0, lb).trimEnd(); // "import"
  const suffix = trimmed.slice(rb); // "} from \"utils\""
  const items = trimmed
    .slice(lb + 1, rb)
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (items.length < 2) return null;

  const cont = tab.repeat(base + 1);
  const out = [tab.repeat(base) + prefix + " {"];
  let cur = cont;
  for (const item of items) {
    const piece = item + ",";
    if (cur.length > cont.length) {
      const candidate = cur + " " + piece;
      if (displayWidth(candidate) > PRINT_WIDTH) {
        out.push(cur);
        cur = cont + piece;
      } else {
        cur = candidate;
      }
    } else {
      cur = cont + piece;
    }
  }
  out.push(cur);
  out.push(tab.repeat(base) + suffix);
  return out;
}

// Split a long binary-operation statement, breaking before its lowest-precedence
// top-level operators (leading-operator continuations at `base + 1`), one per
// line. `null` when there's nothing to break on, brackets are unbalanced, or the
// line is a delicate `if`/`then`/`match`/`await` form.
function splitOperation(trimmed, code, base, tab) {
  const bare = blankStrings(code);
  if (/\b(if|then|else|match|await)\b/.test(bare)) return null;
  if (!isBalanced(code)) return null;
  const ops = topLevelBinops(code);
  if (ops.length === 0) return null;
  const minPrec = Math.min(...ops.map((o) => o.prec));
  const breaks = ops.filter((o) => o.prec === minPrec).map((o) => o.pos);

  const segs = [];
  let start = 0;
  for (const b of breaks) {
    segs.push(trimmed.slice(start, b).trim());
    start = b;
  }
  segs.push(trimmed.slice(start).trim());
  if (segs.length < 2) return null;

  const cont = tab.repeat(base + 1);
  const out = [tab.repeat(base) + segs[0]];
  for (let k = 1; k < segs.length; k++) out.push(cont + segs[k]);
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
