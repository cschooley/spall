// Sensor and dependency chips in the status bar (#16 reworked): taphttp and
// flux live here, not in the suite launcher — a sensor and the findings
// library belong beside the OOB chip, at a glance, clickable for the detail
// spall can get now. The launcher keeps the apps you switch to (spall,
// kudzu, scurf). Headless: a popover of facts, no page. Text only.
import { useEffect, useRef, useState } from "react";
import { apiJson } from "../lib/engagement.ts";
import { asCaInfo, type CaInfo } from "../lib/ca.ts";

type Source = "pending" | "lever" | "log" | "unknown";

/**
 * Toggle a popover that closes on outside click or Escape (as the launcher
 * does). The popover is `position: fixed` so it escapes the status bar's
 * `overflow: hidden`; `pos` places it just above the chip from the button's
 * rect.
 */
function usePopover() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; bottom: number }>({ left: 0, bottom: 0 });
  const root = useRef<HTMLDivElement>(null);
  const place = () => {
    const r = root.current?.getBoundingClientRect();
    if (r) setPos({ left: Math.round(r.left), bottom: Math.round(window.innerHeight - r.top + 6) });
  };
  const toggle = () => {
    if (!open) place();
    setOpen((o) => !o);
  };
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    const onMove = () => setOpen(false); // a scroll or resize drops it rather than letting it drift
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onMove);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onMove);
    };
  }, [open]);
  const style = { left: `${pos.left}px`, bottom: `${pos.bottom}px` };
  return { open, toggle, root, style };
}

export function TaphttpChip({ source, connected, proxy }: { source: Source; connected: boolean | null; proxy: string | null }) {
  const { open, toggle, root, style } = usePopover();
  const [ca, setCa] = useState<CaInfo | string | null>(null);
  const lever = source === "lever";
  useEffect(() => {
    if (!open || !lever || ca !== null) return;
    let live = true;
    void apiJson("/api/ca").then((b) => live && setCa(typeof b === "string" ? b : (asCaInfo(b) ?? "unexpected answer")));
    return () => {
      live = false;
    };
  }, [open, lever, ca]);
  const info = ca && typeof ca !== "string" ? ca : null;
  const dot =
    source === "lever" ? (
      <span className={connected ? "ok" : "bad"}>{connected ? "● connected" : "● disconnected"}</span>
    ) : source === "log" ? (
      <span>● log file</span>
    ) : source === "pending" ? (
      <span>…</span>
    ) : (
      <span className="bad">● unknown</span>
    );
  return (
    <div className="dep" ref={root}>
      <button type="button" className="dep-chip" aria-haspopup="dialog" aria-expanded={open} aria-label="taphttp status" onClick={toggle}>
        {"taphttp "}
        {dot}
        {proxy ? <span className="addr">{` ${proxy}`}</span> : null}
      </button>
      {open ? (
        <div className="dep-pop" role="dialog" aria-label="taphttp details" style={style}>
          <h4>taphttp · the proxy spall reads traffic from</h4>
          <dl>
            <dt>connection</dt>
            <dd>{lever ? (connected ? "lever API, feed connected" : "lever API, feed disconnected") : source === "log" ? "tailing a log file (no history, no replay)" : "unknown"}</dd>
            <dt>proxy</dt>
            <dd>{proxy ?? "unknown"}</dd>
            {info ? (
              <>
                <dt>version</dt>
                <dd>{info.taphttpVersion ?? "unknown"}</dd>
                <dt>CA SHA-256</dt>
                <dd className="fp">{info.sha256 ?? "unknown"}</dd>
                <dt>CA valid until</dt>
                <dd>{info.notAfter ?? "unknown"}</dd>
              </>
            ) : null}
          </dl>
          {typeof ca === "string" ? <p className="note">{`CA: ${ca}`}</p> : null}
          <p className="note">
            The CA key and the API token never leave taphttp. <a href="#engagement/ca">Set up a browser</a>
          </p>
        </div>
      ) : null}
    </div>
  );
}

export function FluxChip({ flux }: { flux: { crate: string; spec: string; rules: string } | null }) {
  const { open, toggle, root, style } = usePopover();
  return (
    <div className="dep" ref={root}>
      <button type="button" className="dep-chip" aria-haspopup="dialog" aria-expanded={open} aria-label="flux status" onClick={toggle}>
        {"flux"}
        {flux ? <span className="addr">{` ${flux.crate}`}</span> : null}
      </button>
      {open ? (
        <div className="dep-pop" role="dialog" aria-label="flux details" style={style}>
          <h4>flux · the findings format (a SARIF 2.1.0 profile)</h4>
          <dl>
            <dt>crate</dt>
            <dd>{flux?.crate ?? "unknown"}</dd>
            <dt>spec</dt>
            <dd>{flux?.spec ?? "unknown"}</dd>
            <dt>rule ids</dt>
            <dd>{flux?.rules ?? "unknown"}</dd>
          </dl>
          <ul className="note">
            <li>A finding is a SARIF result; spall defines no finding type of its own.</li>
            <li>flux/status: candidate, suspected, confirmed, fixed (is it real?).</li>
            <li>Suppressions: rejected, accepted risk, duplicate (what a human decided), each with a justification.</li>
            <li>Fingerprint flux/dast/v1: host, method, path (ids normalized), parameter, rule.</li>
            <li>Redaction before anything leaves spall (export, M5).</li>
          </ul>
          <p className="note">github.com/cschooley/flux</p>
        </div>
      ) : null}
    </div>
  );
}
