// The race-mode gate: `wirescript test --race` must flag the two-handler race
// in tests/race/score_race.ws and pass its single-writer fix. Exit 0 means
// both held.
const { spawnSync } = require("child_process");
const path = require("path");

const exe = process.argv[2];
const dir = path.join(__dirname, "..", "tests", "race");
const run = (file) => spawnSync(exe, ["test", "--race", "50", path.join(dir, file)], { encoding: "utf8" });

const fixed = run("score_single_writer.ws");
const racy = run("score_race.ws");
const flagged = racy.status !== 0 && /race: the result depends on handler order/.test(racy.stdout);
console.log(`single writer: ${fixed.status === 0 ? "passes" : "FAILS"}`);
console.log(`two handlers:  ${flagged ? "flagged as a race" : "NOT flagged"}`);
if (fixed.status !== 0) console.log(fixed.stdout);
if (!flagged) console.log(racy.stdout);
const ok = fixed.status === 0 && flagged;
console.log(ok ? "0 differences" : "race gate failed");
process.exit(ok ? 0 : 1);
