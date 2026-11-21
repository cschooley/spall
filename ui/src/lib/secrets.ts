// Headers whose values are credentials. The list is flux's redaction list
// (docs/flux/SPEC.md, Redaction), so the console highlights exactly what an
// export will scrub. When the flux crate ships, take the list from there.
const SECRET_HEADERS = new Set(["authorization", "cookie", "set-cookie", "proxy-authorization"]);

export function isSecretHeader(name: string): boolean {
  return SECRET_HEADERS.has(name.toLowerCase());
}
