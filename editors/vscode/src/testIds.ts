// The parts of the Testing view that do not need VS Code: test item ids and
// the text a run reports.

/** What `wirescript.runTest` returns. */
export interface Verdict {
  passed: boolean;
  problems: string[];
  lines: { tick: number; text: string }[];
}

/** The label of the item standing for a file's `.expected` comparison. */
export const EXPECTED = "(expected output)";

/** The id of a test item: its file, `#`, its test name. */
export function testId(file: string, name: string): string {
  return `${file}#${name}`;
}

/** The test name a test item runs: `null` for the expected-output item. */
export function testName(id: string): string | null {
  const hash = id.lastIndexOf("#");
  const name = hash < 0 ? "" : id.slice(hash + 1);
  return name === EXPECTED ? null : name;
}

/** The text a run's output gets for one chat line. */
export function outputLine(label: string, tick: number, text: string): string {
  return `[${label}] ${tick}: ${text}\r\n`;
}

/** The failure message for a verdict: one line per problem. */
export function failureText(verdict: Verdict): string {
  return verdict.problems.length > 0 ? verdict.problems.join("\n") : "failed";
}
