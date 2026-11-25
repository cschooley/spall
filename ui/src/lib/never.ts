// The global never-test list (#84): hosts no engagement may reach unless
// that engagement is explicitly exempted from the entry. The server keeps
// and enforces it; this reads and changes it.
import { apiJson } from "./engagement.ts";

export interface NeverList {
  entries: string[];
  /** The active engagement's exceptions (entries it may reach). */
  exceptions: string[];
}

/** Offered, never switched on by default. */
export const SUGGESTED = ["*.gov", "*.mil"];

const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

export function asNever(v: unknown): NeverList | string {
  if (typeof v === "string") return v;
  const o = (v ?? {}) as Record<string, unknown>;
  return { entries: strs(o.entries), exceptions: strs(o.exceptions) };
}

export async function loadNever(): Promise<NeverList | string> {
  return asNever(await apiJson("/api/never-test"));
}

/** Add or remove an entry; the new entries, or why not. */
export async function changeNever(op: "add" | "remove", entry: string): Promise<string[] | string> {
  const v = await apiJson("/api/never-test", { method: "POST", body: { op, entry } });
  if (typeof v === "string") return v;
  return strs((v as Record<string, unknown>).entries);
}

/** Exempt the active engagement from `entry`, or not; the new exceptions, or why not. */
export async function setException(list: NeverList, entry: string, on: boolean): Promise<string | null> {
  const next = on ? [...list.exceptions, entry] : list.exceptions.filter((x) => x !== entry);
  const v = await apiJson("/api/engagement", { method: "PATCH", body: { never_test_exceptions: next } });
  return typeof v === "string" ? v : null;
}
