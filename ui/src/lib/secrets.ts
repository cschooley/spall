// Headers whose values are credentials: flux's redaction list (#49), so the
// inspector highlights exactly what an export scrubs. spall serves the list
// from the flux crate (/api/redaction); this default is the same list, for
// before it loads or when it can't.
let secretHeaders = new Set(["authorization", "proxy-authorization", "cookie", "set-cookie", "x-api-key", "api-key", "x-auth-token", "x-access-token"]);

export function isSecretHeader(name: string): boolean {
  return secretHeaders.has(name.toLowerCase());
}

/** Use flux's list from /api/redaction (`{"headers": [...]}`). Ignores anything else. */
export function setSecretHeaders(v: unknown): boolean {
  const h = v && typeof v === "object" ? (v as Record<string, unknown>).headers : null;
  if (!Array.isArray(h) || h.length === 0 || !h.every((x) => typeof x === "string")) return false;
  secretHeaders = new Set((h as string[]).map((x) => x.toLowerCase()));
  return true;
}
