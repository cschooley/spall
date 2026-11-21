// Suite launcher (9-dot menu), as in the verdigris mockup. Tools that aren't
// built or integrated yet are placeholders: shown, light off, disabled. Each
// becomes live when it ships. The light shows a tool's reachability; only
// spall itself and taphttp (via /api/status) are known today.
import { useEffect, useRef, useState } from "react";
import { Icon, type IconName } from "./Icon.tsx";

export interface Tile {
  id: string;
  icon: IconName;
  /** "here": this app; "app": another suite app; "headless": status only; "library": no service. */
  kind: "here" | "app" | "headless" | "library";
  up: boolean;
  /** Placeholder until the tool (or spall's integration with it) ships. */
  placeholder: boolean;
  note: string;
}

export function suiteTiles(taphttpConnected: boolean | null): Tile[] {
  return [
    { id: "spall", icon: "icons/spall", kind: "here", up: true, placeholder: false, note: "this app" },
    { id: "kudzu", icon: "icons/kudzu", kind: "app", up: false, placeholder: true, note: "not built yet" },
    { id: "scurf", icon: "icons/scurf", kind: "app", up: false, placeholder: true, note: "not built yet" },
    {
      id: "taphttp",
      icon: "icons/taphttp",
      kind: "headless",
      up: taphttpConnected === true,
      placeholder: false,
      note: taphttpConnected === true ? "headless: status only · connected" : taphttpConnected === false ? "headless: status only · disconnected" : "headless: status only · log file",
    },
    { id: "flux", icon: "icons/flux", kind: "library", up: false, placeholder: false, note: "library: no service" },
  ];
}

export function Launcher({ taphttpConnected }: { taphttpConnected: boolean | null }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className={open ? "launch open" : "launch"} ref={root}>
      <button type="button" aria-label="Switch tool" aria-haspopup="true" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <span className="grid9" aria-hidden>
          {Array.from({ length: 9 }, (_, i) => (
            <i key={i} />
          ))}
        </span>
      </button>
      {open ? (
        <div className="panel" role="group" aria-label="Suite tools">
          {suiteTiles(taphttpConnected).map((t) => (
            <span
              key={t.id}
              data-tool={t.id}
              className={["tile", t.kind === "here" ? "here" : "", t.kind === "headless" || t.kind === "library" ? "headless" : "", t.placeholder ? "todo" : ""].filter(Boolean).join(" ")}
              aria-current={t.kind === "here" ? "true" : undefined}
              aria-disabled={t.kind === "here" ? undefined : "true"}
              title={`${t.id}: ${t.note}`}
            >
              <span className={t.up ? "led up" : "led"} aria-label={t.up ? "up" : "off"} />
              <Icon name={t.icon} size={22} />
              {t.id}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  );
}
