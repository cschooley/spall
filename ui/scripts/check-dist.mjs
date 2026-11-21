// Post-build guard: the built page must work under spall's CSP
// (script-src 'self'; style-src 'self'; no inline anything, no CDN).
// Fails the build instead of failing silently in the browser.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** Problems with a built index.html, as human-readable strings. */
export function checkHtml(html) {
  const problems = [];
  for (const m of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
    if (!/\bsrc\s*=/.test(m[1])) problems.push("inline <script> (CSP blocks it)");
    else if (m[2].trim() !== "") problems.push("<script src> with inline content");
  }
  if (/<style\b/i.test(html)) problems.push("inline <style> (CSP blocks it)");
  if (/\sstyle\s*=/i.test(html)) problems.push("style= attribute (CSP blocks it)");
  if (/\son[a-z]+\s*=/i.test(html)) problems.push("inline event handler (CSP blocks it)");
  for (const m of html.matchAll(/\b(?:src|href)\s*=\s*["']?((?:https?:)?\/\/[^"'\s>]+)/gi)) {
    problems.push(`external URL ${m[1]} (no CDN: bundle it)`);
  }
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const file = new URL("../dist/index.html", import.meta.url);
  const problems = checkHtml(readFileSync(file, "utf8"));
  if (problems.length) {
    console.error("dist/index.html violates spall's CSP rules:\n  " + problems.join("\n  "));
    process.exit(1);
  }
  console.log("dist/index.html: CSP check ok");
}
