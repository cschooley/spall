// The CA's public details for browser setup (#32), from spall's /api/ca.

export interface CaInfo {
  sha256: string | null;
  subject: string | null;
  notBefore: string | null;
  notAfter: string | null;
  proxy: string | null;
  /** taphttp's own version, from its /info (#16). */
  taphttpVersion: string | null;
}

const s = (v: unknown) => (typeof v === "string" && v ? v : null);

export function asCaInfo(v: unknown): CaInfo | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  return { sha256: s(o.sha256), subject: s(o.subject), notBefore: s(o.not_before), notAfter: s(o.not_after), proxy: s(o.proxy), taphttpVersion: s(o.taphttp_version) };
}
