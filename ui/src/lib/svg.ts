// Parse a vendored verdigris icon into plain data React can render, so no
// markup string ever reaches innerHTML (the "never HTML" lint ban has no
// exception, even for our own files). Deliberately strict: only shape
// elements and presentation attributes; anything else throws.

const SHAPES = new Set(["path", "circle", "rect", "line", "polyline", "polygon", "ellipse", "g"]);
const ATTRS = new Set([
  "viewBox", "d", "cx", "cy", "r", "rx", "ry", "x", "y", "x1", "y1", "x2", "y2", "width", "height", "points",
  "fill", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin", "stroke-dasharray", "opacity", "transform",
]);
/** Root attributes that are sizing or noise; the component sets its own. */
const ROOT_SKIP = new Set(["xmlns", "width", "height", "aria-hidden"]);

export interface IconNode {
  tag: string;
  attrs: Record<string, string>;
}
export interface Icon {
  attrs: Record<string, string>;
  children: IconNode[];
}

const camel = (name: string) => name.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase());

function attrsOf(raw: string, root: boolean): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /([A-Za-z_:][-A-Za-z0-9_:.]*)\s*=\s*"([^"]*)"/g;
  let rest = raw;
  for (const m of raw.matchAll(re)) {
    const [, name, value] = m as unknown as [string, string, string];
    rest = rest.replace(m[0], "");
    if (root && ROOT_SKIP.has(name)) continue;
    if (!ATTRS.has(name)) throw new Error(`icon: attribute ${name} not allowed`);
    if (/url\(|javascript:|expression\(/i.test(value)) throw new Error(`icon: ${name} value not allowed`);
    out[camel(name)] = value;
  }
  if (rest.replace(/\/$/, "").trim() !== "") throw new Error(`icon: unparsed attributes: ${rest.trim()}`);
  return out;
}

export function parseIcon(svg: string): Icon {
  const root = /^\s*<svg\b([^>]*)>([\s\S]*)<\/svg>\s*$/.exec(svg);
  if (!root) throw new Error("icon: not a single <svg> element");
  const attrs = attrsOf(root[1]!, true);
  let body = root[2]!.replace(/<title>[^<]*<\/title>/g, "");
  const children: IconNode[] = [];
  const el = /<([a-zA-Z]+)\b([^>]*?)\/>/g;
  for (const m of body.matchAll(el)) {
    const tag = m[1]!;
    if (!SHAPES.has(tag)) throw new Error(`icon: element <${tag}> not allowed`);
    children.push({ tag, attrs: attrsOf(m[2]!, false) });
    body = body.replace(m[0], "");
  }
  if (body.trim() !== "") throw new Error(`icon: unexpected content: ${body.trim().slice(0, 40)}`);
  return { attrs, children };
}
