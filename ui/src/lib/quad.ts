// Quad mode (#66): what /api/status says about it, and the calls. A burst
// is 4x agent workers at 4x rate inside the RoE and budgets; it wears off
// on its own, the e-stop ends it, and its scorecard is numbers only (the
// server builds the shareable card from numbers; nothing here adds words).
import { apiJson } from "./engagement.ts";

export interface Quad {
  until: string;
  seconds: number;
  workers: number;
  /** Why 4x is less than 4x (the RoE's ceiling), if it is. */
  capped: string | null;
}

export interface Scorecard {
  seconds: number;
  workers: number;
  requests: number;
  tokens: number;
  candidates: number;
  ended: "time" | "stopped" | "estop";
  /** The shareable card, numbers only. */
  card: string;
}

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null);

export function asQuad(v: unknown): Quad | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const seconds = num(o.seconds);
  const workers = num(o.workers);
  if (typeof o.until !== "string" || seconds === null || workers === null) return null;
  return { until: o.until, seconds, workers, capped: typeof o.capped === "string" ? o.capped : null };
}

export function asScorecard(v: unknown): Scorecard | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const n = ["seconds", "workers", "requests", "tokens", "candidates"].map((k) => num(o[k]));
  const ended = o.ended === "time" || o.ended === "stopped" || o.ended === "estop" ? o.ended : null;
  if (n.some((x) => x === null) || ended === null || typeof o.card !== "string") return null;
  const [seconds, workers, requests, tokens, candidates] = n as number[];
  return { seconds: seconds!, workers: workers!, requests: requests!, tokens: tokens!, candidates: candidates!, ended, card: o.card };
}

/** `m:ss`. */
export function mmss(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Seconds left of a burst at `now` (ms). */
export function remaining(q: Quad, now: number): number {
  const until = Date.parse(q.until);
  return Number.isNaN(until) ? 0 : Math.max(0, Math.ceil((until - now) / 1000));
}

/** The scorecard as console lines. */
export function scoreLines(c: Scorecard): string[] {
  const why = c.ended === "estop" ? " (e-stop)" : c.ended === "stopped" ? " (ended early)" : "";
  return [`quad over${why}: ${mmss(c.seconds)}, ${c.workers} workers`, `  ${c.requests} requests · ${c.tokens} tokens · ${c.candidates} candidates`, `  card: ${c.card}`];
}

/** Start a burst (`seconds` default 120) or end it; what happened, as console lines. */
export async function quadCall(what: { seconds?: number } | { stop: true }): Promise<{ ok: boolean; lines: string[] }> {
  const body = await apiJson("/api/quad", { method: "POST", body: what });
  if (typeof body === "string") return { ok: false, lines: [`quad: ${body}`] };
  const o = body as { quad?: unknown; scorecard?: unknown };
  const card = asScorecard(o.scorecard);
  if (card) return { ok: true, lines: scoreLines(card) };
  const q = asQuad(o.quad);
  if (!q) return { ok: false, lines: ["quad: unexpected answer from spall"] };
  const lines = [`QUAD for ${mmss(q.seconds)}: ${q.workers} agent workers, 4x rate, inside the rules of engagement and budgets`];
  if (q.capped) lines.push(`  ${q.capped}`);
  return { ok: true, lines };
}
