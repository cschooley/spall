// Suite launcher (9-dot menu), as in the verdigris mockup. Tools that aren't
// built or integrated yet are placeholders: shown, light off, disabled. Each
// becomes live when it ships. The light shows a tool's reachability; only
// spall itself and taphttp (via /api/status) are known today.
//
// The headless tiles (#16, decided: details in place, no new page): taphttp
// (a service with no UI) shows its connection, version and CA fingerprint;
// flux (a library) shows the crate and spec spall is built with and what
// the profile means. Text only; nothing is fetched from outside spall.
import { useEffect, useRef, useState } from "react";
import { Icon, type IconName } from "./Icon.tsx";
import { apiJson } from "../lib/engagement.ts";
import { asCaInfo, type CaInfo } from "../lib/ca.ts";

export interface LauncherStatus {
  taphttpConnected: boolean | null;
  source: "pending" | "lever" | "log" | "unknown";
  proxy: string | null;
  flux: { crate: string; spec: string; rules: string } | null;
}

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

export function Launcher({ status }: { status: LauncherStatus }) {
  const { taphttpConnected } = status;
  const [open, setOpen] = useState(false);
  const [details, setDetails] = useState<"taphttp" | "flux" | null>(null);
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
          {suiteTiles(taphttpConnected).map((t) => {
            const cls = ["tile", t.kind === "here" ? "here" : "", t.kind === "headless" || t.kind === "library" ? "headless" : "", t.placeholder ? "todo" : "", details === t.id ? "on" : ""]
              .filter(Boolean)
              .join(" ");
            const inner = (
              <>
                <span className={t.up ? "led up" : "led"} aria-label={t.up ? "up" : "off"} />
                <Icon name={t.icon} size={22} />
                {t.id}
              </>
            );
            return t.id === "taphttp" || t.id === "flux" ? (
              <button
                key={t.id}
                type="button"
                data-tool={t.id}
                className={cls}
                aria-expanded={details === t.id}
                title={`${t.id}: ${t.note}`}
                onClick={() => setDetails((d) => (d === t.id ? null : (t.id as "taphttp" | "flux")))}
              >
                {inner}
              </button>
            ) : (
              <span
                key={t.id}
                data-tool={t.id}
                className={cls}
                aria-current={t.kind === "here" ? "true" : undefined}
                aria-disabled={t.kind === "here" ? undefined : "true"}
                title={`${t.id}: ${t.note}`}
              >
                {inner}
              </span>
            );
          })}
          {details === "taphttp" ? <TaphttpDetails status={status} /> : null}
          {details === "flux" ? <FluxDetails flux={status.flux} /> : null}
        </div>
      ) : null}
    </div>
  );
}

function TaphttpDetails({ status }: { status: LauncherStatus }) {
  const [ca, setCa] = useState<CaInfo | string | null>(null);
  const lever = status.source === "lever";
  useEffect(() => {
    if (!lever) return;
    let live = true;
    void apiJson("/api/ca").then((b) => live && setCa(typeof b === "string" ? b : (asCaInfo(b) ?? "unexpected answer")));
    return () => {
      live = false;
    };
  }, [lever]);
  const info = ca && typeof ca !== "string" ? ca : null;
  return (
    <section className="tool-details" aria-label="taphttp details">
      <h4>taphttp · the proxy spall reads traffic from</h4>
      <dl>
        <dt>connection</dt>
        <dd>{lever ? (status.taphttpConnected ? "lever API, feed connected" : "lever API, feed disconnected") : status.source === "log" ? "tailing a log file (no history, no replay)" : "unknown"}</dd>
        <dt>proxy</dt>
        <dd>{status.proxy ?? "unknown"}</dd>
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
    </section>
  );
}

function FluxDetails({ flux }: { flux: LauncherStatus["flux"] }) {
  return (
    <section className="tool-details" aria-label="flux details">
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
    </section>
  );
}
