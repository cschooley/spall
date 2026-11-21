// verdigris icons, rendered from the vendored SVG files through a strict
// parser (lib/svg.ts): no markup string reaches the DOM. Every icon carries
// data-icon so tests can tell spall's own SVGs from injected ones.
import { createElement } from "react";
import { parseIcon, type Icon as IconData } from "../lib/svg.ts";

const raw = import.meta.glob<string>("../vendor/verdigris/icons/{,views/}*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
});

const ICONS: Record<string, IconData> = Object.fromEntries(
  Object.entries(raw).map(([path, svg]) => [path.replace(/^.*\/vendor\/verdigris\/(.+)\.svg$/, "$1"), parseIcon(svg)]),
);

export type IconName =
  | "icons/spall" | "icons/kudzu" | "icons/scurf" | "icons/taphttp" | "icons/flux" | "icons/verdigris"
  | "icons/views/engagement" | "icons/views/traffic" | "icons/views/surface" | "icons/views/tasks" | "icons/views/findings";

export function Icon({ name, size = 20 }: { name: IconName; size?: number }) {
  const icon = ICONS[name];
  if (!icon) return null;
  return createElement(
    "svg",
    { ...icon.attrs, width: size, height: size, "aria-hidden": true, focusable: "false", "data-icon": name },
    icon.children.map((c, i) => createElement(c.tag, { key: i, ...c.attrs })),
  );
}
