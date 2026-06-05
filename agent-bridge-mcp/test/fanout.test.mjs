// test/fanout.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { fanOut } from "../src/fanout.mjs";

test("fanOut delivers the message to every sink", async () => {
  const seen = [];
  const sinks = [
    { name: "a", deliver: (m) => seen.push(`a:${m.id}`) },
    { name: "b", deliver: (m) => seen.push(`b:${m.id}`) },
  ];
  await fanOut({ id: "m1" }, sinks);
  assert.deepEqual(seen.sort(), ["a:m1", "b:m1"]);
});

test("fanOut isolates a throwing sink — others still receive + it is logged", async () => {
  const seen = [];
  const logs = [];
  const sinks = [
    { name: "bad", deliver: () => { throw new Error("boom"); } },
    { name: "good", deliver: (m) => seen.push(m.id) },
  ];
  await fanOut({ id: "m1" }, sinks, (s) => logs.push(s));
  assert.deepEqual(seen, ["m1"]);
  assert.equal(logs.length, 1);
  assert.match(logs[0], /sink "bad" failed: boom/);
});

test("fanOut on no sinks is a no-op", async () => {
  await fanOut({ id: "m1" }, undefined); // must not throw
  await fanOut({ id: "m1" }, []);
});
