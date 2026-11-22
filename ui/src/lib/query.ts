// The filter bar's syntax: `host:x method:POST status:404|4xx replay:yes
// free text`. Filters and free text go to taphttp's GET /exchanges (via
// spall); the same predicate runs client-side on live rows, mirroring the
// API's semantics so both agree. `replay:` is client-side only: taphttp
// filters on one original (`replay_of=<id>`), not on being a replay.
// `scope:` is client-side too, from spall's host states (#39).

export interface Query {
  host?: string;
  method?: string;
  status?: number;
  statusClass?: number;
  text?: string;
  /** replay:yes (only replays) or replay:no (only originals). */
  replay?: boolean;
  /** scope:in|out|unknown, from the server's host states (#39). */
  scope?: "in" | "out" | "unknown";
  /** Tokens that look like filters but aren't supported (e.g. by:agent, M5). */
  ignored: string[];
}

export function parseFilter(input: string): Query {
  const q: Query = { ignored: [] };
  const words: string[] = [];
  for (const tok of input.trim().split(/\s+/).filter(Boolean)) {
    const m = /^([a-z]+):(.*)$/i.exec(tok);
    if (!m) {
      words.push(tok);
      continue;
    }
    const key = m[1]!.toLowerCase();
    const val = m[2]!;
    if (key === "host" && val) q.host = val;
    else if (key === "method" && /^[a-z]+$/i.test(val)) q.method = val.toUpperCase();
    else if (key === "status" && /^[1-5]\d\d$/.test(val)) q.status = Number(val);
    else if (key === "status" && /^[1-5]xx$/i.test(val)) q.statusClass = Number(val[0]);
    else if (key === "replay" && /^(yes|no)$/i.test(val)) q.replay = val.toLowerCase() === "yes";
    else if (key === "scope" && /^(in|out|unknown)$/i.test(val)) q.scope = val.toLowerCase() as Query["scope"];
    else if (["host", "method", "status", "by", "replay", "scope"].includes(key)) q.ignored.push(tok);
    else words.push(tok); // e.g. "https://..." in free text
  }
  if (words.length) q.text = words.join(" ");
  return q;
}

/** Query params for spall's /api/exchanges (taphttp's GET /exchanges). */
export function apiParams(q: Query): [string, string][] {
  const p: [string, string][] = [];
  if (q.host) p.push(["host", q.host]);
  if (q.method) p.push(["method", q.method]);
  if (q.status !== undefined) p.push(["status", String(q.status)]);
  else if (q.statusClass !== undefined) p.push(["status", `${q.statusClass}xx`]);
  if (q.text) p.push(["q", q.text]);
  return p;
}

export function matchesQuery(
  e: { host: string; method: string; url: string; status: number | null; replay_of?: string | null },
  q: Query,
  /** A host's scope state, for scope:; a host the server hasn't seen yet is unknown. */
  stateOf: (host: string) => "in" | "out" | "unknown" | null = () => null,
): boolean {
  const lc = (s: string) => s.toLowerCase();
  if (q.host && !lc(e.host).includes(lc(q.host))) return false;
  if (q.method && e.method.toUpperCase() !== q.method) return false;
  if (q.status !== undefined && e.status !== q.status) return false;
  if (q.statusClass !== undefined && (e.status === null || Math.floor(e.status / 100) !== q.statusClass)) return false;
  if (q.text && !lc(e.url).includes(lc(q.text)) && !lc(e.host).includes(lc(q.text))) return false;
  if (q.replay !== undefined && Boolean(e.replay_of) !== q.replay) return false;
  if (q.scope !== undefined && (stateOf(e.host) ?? "unknown") !== q.scope) return false;
  return true;
}

export function isEmptyQuery(q: Query): boolean {
  return apiParams(q).length === 0 && q.replay === undefined && q.scope === undefined;
}
