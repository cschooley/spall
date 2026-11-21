// The console's scrollback: commands, their output, and system messages.
// Pure state updates; rendering is text only.
import type { OutKind } from "../commands/registry.ts";

export const MAX_LINES = 1000;

export interface Line {
  id: number;
  text: string;
  kind: OutKind;
}
export interface ConsoleLog {
  lines: Line[];
  unread: number;
  next: number;
}

export function emptyLog(): ConsoleLog {
  return { lines: [], unread: 0, next: 1 };
}

/** Append a line. Output arriving while the console is closed counts as unread. */
export function addLine(log: ConsoleLog, text: string, kind: OutKind, open: boolean): ConsoleLog {
  const lines = log.lines.concat({ id: log.next, text, kind });
  return {
    lines: lines.length > MAX_LINES ? lines.slice(lines.length - MAX_LINES) : lines,
    unread: !open && kind !== "cmd" ? log.unread + 1 : log.unread,
    next: log.next + 1,
  };
}

export function markRead(log: ConsoleLog): ConsoleLog {
  return log.unread === 0 ? log : { ...log, unread: 0 };
}

export function clearLog(log: ConsoleLog): ConsoleLog {
  return { lines: [], unread: 0, next: log.next };
}
