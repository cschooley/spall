import { test } from "node:test";
import assert from "node:assert/strict";
import { addLine, clearLog, emptyLog, markRead, MAX_LINES } from "./consoleLog.ts";

test("lines append in order with unique ids", () => {
  let l = addLine(emptyLog(), "a", "sys", true);
  l = addLine(l, "b", "out", true);
  assert.deepEqual(l.lines.map((x) => [x.text, x.kind]), [["a", "sys"], ["b", "out"]]);
  assert.notEqual(l.lines[0]!.id, l.lines[1]!.id);
});

test("system output while closed counts as unread; echoed commands don't", () => {
  let l = addLine(emptyLog(), "boot", "sys", false);
  l = addLine(l, "x", "warn", false);
  l = addLine(l, "typed", "cmd", false);
  assert.equal(l.unread, 2);
  assert.equal(markRead(l).unread, 0);
  assert.equal(addLine(markRead(l), "y", "sys", true).unread, 0, "open: nothing unread");
});

test("bounded scrollback; clear empties lines and unread", () => {
  let l = emptyLog();
  for (let i = 0; i < MAX_LINES + 10; i++) l = addLine(l, String(i), "out", true);
  assert.equal(l.lines.length, MAX_LINES);
  assert.equal(l.lines[0]!.text, "10");
  assert.deepEqual(clearLog(addLine(l, "w", "warn", false)).lines, []);
  assert.equal(clearLog(addLine(l, "w", "warn", false)).unread, 0);
});
