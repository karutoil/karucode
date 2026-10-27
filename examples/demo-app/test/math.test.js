import test from "node:test";
import assert from "node:assert/strict";
import { add, clamp, average } from "../src/math.js";

test("add adds", () => {
  assert.equal(add(2, 3), 5);
});

test("add handles negatives", () => {
  assert.equal(add(-2, 3), 1);
});

test("clamp bounds", () => {
  assert.equal(clamp(11, 0, 10), 10);
  assert.equal(clamp(-1, 0, 10), 0);
  assert.equal(clamp(5, 0, 10), 5);
});

test("average of an empty list is 0", () => {
  assert.equal(average([]), 0);
});

test("average rounds nothing", () => {
  assert.equal(average([1, 2, 3]), 2);
});
