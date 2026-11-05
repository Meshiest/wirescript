import { test } from "node:test";
import * as assert from "node:assert/strict";
import { EXPECTED, failureText, outputLine, testId, testName } from "./testIds";

test("a test item id names its test", () => {
  assert.equal(testName(testId("file:///a/b.ws", "adds")), "adds");
});

test("the expected-output item runs the whole program", () => {
  assert.equal(testName(testId("file:///a/b.ws", EXPECTED)), null);
});

test("a file uri with a fragment still yields the test name", () => {
  assert.equal(testName(testId("file:///a/c%23d.ws", "x")), "x");
});

test("output lines carry the test and tick", () => {
  assert.equal(outputLine("adds", 3, "hi"), "[adds] 3: hi\r\n");
});

test("a failure lists every problem", () => {
  assert.equal(
    failureText({ passed: false, problems: ["FAIL: a", "error: b"], lines: [] }),
    "FAIL: a\nerror: b",
  );
  assert.equal(failureText({ passed: false, problems: [], lines: [] }), "failed");
});
