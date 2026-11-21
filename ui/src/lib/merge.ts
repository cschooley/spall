// Combine history pages (from /api/exchanges, newest first) with live SSE
// entries into one oldest-first list.

/** Sortable key for an RFC 3339 UTC time with any fraction length. */
export function tsKey(ts: string): string {
  const m = /^(.*T\d\d:\d\d:\d\d)(?:\.(\d+))?Z$/.exec(ts);
  if (!m) return ts;
  return `${m[1]}.${(m[2] ?? "").padEnd(9, "0").slice(0, 9)}Z`;
}

export function mergeExchanges<T extends { id: string; ts: string }>(
  history: readonly T[],
  live: readonly T[],
  keep: (e: T) => boolean,
): T[] {
  const seen = new Set<string>();
  const out: { e: T; key: string; n: number }[] = [];
  // History arrives newest first; walk it oldest first so ties stay stable.
  const ordered = [...history].reverse().concat(live);
  ordered.forEach((e, n) => {
    if (seen.has(e.id) || !keep(e)) return;
    seen.add(e.id);
    out.push({ e, key: tsKey(e.ts), n });
  });
  out.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : a.n - b.n));
  return out.map((x) => x.e);
}
