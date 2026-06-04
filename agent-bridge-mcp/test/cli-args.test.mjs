// test/cli-args.test.mjs — the air-msg CLI's argument routing + body rendering.
//
// This surface had ZERO coverage before: core.* is well-tested, but the human-facing CLI
// parser was untested, which let a real bug ship — `room create --name X` (and --kind /
// --mandate / --limit) were swallowed by the top-level parse before the room handler saw
// them, so the documented flag form silently did nothing. These tests pin the fix.
import { test } from "node:test";
import assert from "node:assert/strict";
import { parseArgs, parseRoomArgs, bodyText } from "../src/cli.mjs";

test("parseArgs splits positionals from --flag/--flag-value pairs", () => {
  const { positionals, flags } = parseArgs(["send", "bob", "--plaintext"]);
  assert.deepEqual(positionals, ["send", "bob"]);
  assert.equal(flags.plaintext, true);
});

test("room create --name reaches the room handler (regression)", () => {
  const { sub, flags } = parseRoomArgs(["create", "--name", "dryrun"]);
  assert.equal(sub, "create");
  assert.equal(flags.name, "dryrun");
});

test("room create <positional name> still works (no --name)", () => {
  const { sub, positionals, flags } = parseRoomArgs(["create", "dryrun"]);
  assert.equal(sub, "create");
  assert.equal(flags.name, undefined);
  assert.equal(positionals.join(" "), "dryrun");
});

test("room invite carries --kind and --mandate through to the handler", () => {
  const { sub, positionals, flags } = parseRoomArgs(["invite", "ROOM", "AIR-B", "--kind", "human", "--mandate", "m1"]);
  assert.equal(sub, "invite");
  assert.deepEqual(positionals, ["ROOM", "AIR-B"]);
  assert.equal(flags.kind, "human");
  assert.equal(flags.mandate, "m1");
});

test("room send keeps the message text as positionals", () => {
  const { sub, positionals } = parseRoomArgs(["send", "ROOM", "hello", "room", "world"]);
  assert.equal(sub, "send");
  assert.equal(positionals[0], "ROOM");
  assert.equal(positionals.slice(1).join(" "), "hello room world");
});

test("room history --limit reaches the handler", () => {
  const { sub, positionals, flags } = parseRoomArgs(["history", "ROOM", "--limit", "10"]);
  assert.equal(sub, "history");
  assert.equal(positionals[0], "ROOM");
  assert.equal(flags.limit, "10");
});

test("bodyText renders a room/msg as its text, not a raw JSON blob", () => {
  assert.equal(bodyText({ type: "room/msg", text: "hi room", recipients: [], mentions: [] }), "hi room");
});

test("bodyText renders a room/joined system notice", () => {
  assert.equal(
    bodyText({ type: "room/joined", room_id: "r", room_name: "dryrun" }),
    `📥 You were added to room "dryrun"`,
  );
});

test("bodyText still renders plain text + falls back to JSON for unknown types", () => {
  assert.equal(bodyText({ type: "text", text: "hello" }), "hello");
  assert.equal(bodyText({ type: "weird", x: 1 }), JSON.stringify({ type: "weird", x: 1 }));
  assert.equal(bodyText(null), "");
});
