// Hosts seen in traffic (#39): spall's /api/hosts says each host's scope
// state (from the server's scope engine). The UI only looks hosts up; it
// never decides scope itself.

export type HostState = "in" | "out" | "unknown";

export interface HostRow {
  host: string;
  count: number;
  firstSeen: string;
  lastSeen: string;
  state: HostState;
  by: string | null;
}

export interface HostListing {
  hosts: HostRow[];
  counts: { in: number; out: number; unknown: number };
  dropped: number;
}

const stateOf = (v: unknown): HostState => (v === "in" || v === "out" ? v : "unknown");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);

export function asListing(v: unknown): HostListing | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (!Array.isArray(o.hosts)) return null;
  const hosts = o.hosts.flatMap((x: unknown): HostRow[] => {
    const h = x as Record<string, unknown> | null;
    if (!h || typeof h.host !== "string") return [];
    return [
      {
        host: h.host,
        count: num(h.count),
        firstSeen: typeof h.first_seen === "string" ? h.first_seen : "",
        lastSeen: typeof h.last_seen === "string" ? h.last_seen : "",
        state: stateOf(h.state),
        by: typeof h.by === "string" ? h.by : null,
      },
    ];
  });
  const c = (o.counts ?? {}) as Record<string, unknown>;
  return { hosts, counts: { in: num(c.in), out: num(c.out), unknown: num(c.unknown) }, dropped: num(o.dropped) };
}

/** A row's host as the server keys it: lowercase, no port, brackets or trailing dot. */
export function hostKey(host: string): string {
  let h = host.trim().toLowerCase();
  if (h.startsWith("[")) h = h.slice(1, h.indexOf("]") > 0 ? h.indexOf("]") : undefined);
  else {
    const i = h.lastIndexOf(":");
    if (i > 0 && h.indexOf(":") === i && /^\d+$/.test(h.slice(i + 1))) h = h.slice(0, i);
  }
  return h.replace(/\.+$/, "");
}

/** `*.parent` for a host with three or more labels (not an IP), else null. */
export function parentWildcard(host: string): string | null {
  if (/^[\d.]+$/.test(host) || host.includes(":")) return null;
  const labels = host.split(".");
  return labels.length >= 3 ? `*.${labels.slice(1).join(".")}` : null;
}

/** Look up a host's state; null while the server hasn't seen it. */
export function stateLookup(l: HostListing | null): (host: string) => HostRow | null {
  const m = new Map((l?.hosts ?? []).map((h) => [h.host, h]));
  return (host) => m.get(hostKey(host)) ?? null;
}
