// The shared right-hand inspector (docs/ux.md: "One inspector"). M1 shows
// one exchange: request/response tabs, headers, bodies, copy as curl, save
// body. Replay (M2) and Flag candidate (M3) land here later, not as row
// actions. Every value is attacker-controlled and rendered as text.

import { Fragment, useState } from "react";
import { Body } from "./Body.tsx";
import {
  bodyBytes,
  displayUrl,
  durationText,
  headerRows,
  headerValue,
  humanSize,
  methodClass,
  shortId,
  statusClass,
  statusLine,
} from "../lib/format.ts";
import { toCurl } from "../lib/curl.ts";
import { isSecretHeader } from "../lib/secrets.ts";
import type { Header, TrafficEntry } from "../types.ts";

type Tab = "request" | "response";

export function Inspector({ entry, onClose }: { entry: TrafficEntry; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>("request");
  const [copied, setCopied] = useState<"idle" | "ok" | "failed">("idle");
  const [idCopied, setIdCopied] = useState(false);

  const copyCurl = async () => {
    try {
      await navigator.clipboard.writeText(toCurl(entry));
      setCopied("ok");
    } catch {
      setCopied("failed");
    }
    setTimeout(() => setCopied("idle"), 1500);
  };

  const isReq = tab === "request";
  const body = isReq ? entry.req_body : entry.res_body;
  const encoding = isReq ? entry.req_body_encoding : entry.res_body_encoding;
  const headers = isReq ? entry.req_headers : entry.res_headers;
  const bytes = bodyBytes(body, encoding);
  const save = () => bytes && saveBytes(bytes, `spall-${safeName(entry.id)}-${tab}.bin`);
  const type = headerValue(headers, "content-type");

  return (
    <aside className="inspector" aria-label="Exchange inspector">
      <header>
        <h2 title={entry.url}>
          <span className={methodClass(entry.method)}>{entry.method}</span> {displayUrl(entry.url)}
        </h2>
        <div className="meta">
          <button
            type="button"
            className="link id"
            title={`${entry.id} (click to copy)`}
            aria-label={`Copy exchange id ${entry.id}`}
            onClick={() =>
              navigator.clipboard.writeText(entry.id).then(
                () => {
                  setIdCopied(true);
                  setTimeout(() => setIdCopied(false), 1500);
                },
                () => undefined,
              )
            }
          >
            {idCopied ? "id copied" : `id ${shortId(entry.id)}`}
          </button>
          <span className={statusClass(entry.status)}>{statusLine(entry.status)}</span>
          {entry.error ? <span className="error">{entry.error}</span> : null}
          {entry.duration_ms !== null ? <span>{durationText(entry.duration_ms)}</span> : null}
          {entry.replay_of ? <span>{`replay of ${entry.replay_of}`}</span> : null}
        </div>
        <button type="button" className="close" aria-label="Close inspector" title="Close (Esc)" onClick={onClose}>
          ×
        </button>
      </header>
      <div className="actions">
        <button type="button" onClick={copyCurl}>
          {copied === "ok" ? "copied" : copied === "failed" ? "copy failed" : "Copy as curl"}
        </button>
        <button
          type="button"
          disabled={!bytes}
          onClick={save}
        >
          {`Save ${tab} body`}
        </button>
      </div>
      <div className="tabs" role="tablist">
        {(["request", "response"] as const).map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} className={tab === t ? "on" : undefined} onClick={() => setTab(t)}>
            {t}
          </button>
        ))}
      </div>
      <section className="pane" role="tabpanel" aria-label={tab}>
        <h3>
          Headers <span className="note">· wire order, repeats kept</span>
        </h3>
        <Headers headers={headers} />
        <h3>
          Body
          {bytes ? <span className="note">{` · ${type ?? "no content-type"} · ${humanSize(bytes.length)}`}</span> : null}
        </h3>
        <Body body={body} encoding={encoding} headers={headers} onSave={save} />
      </section>
    </aside>
  );
}

/** Credential header values are highlighted and shortened until revealed. */
const SECRET_PREVIEW = 24;

function Headers({ headers }: { headers: Header[] }) {
  const rows = headerRows(headers);
  const [revealed, setRevealed] = useState<ReadonlySet<number>>(new Set());
  if (rows.length === 0) return <p className="none">(none)</p>;
  return (
    <dl className="kv">
      {rows.map((r, i) => {
        const secret = isSecretHeader(r.name);
        const long = secret && r.value.length > SECRET_PREVIEW;
        const shown = long && !revealed.has(i) ? `${r.value.slice(0, SECRET_PREVIEW)}…` : r.value;
        return (
          <Fragment key={i}>
            <dt>{r.name}</dt>
            <dd className={secret ? "secret" : undefined}>
              {shown}
              {r.count ? <span className="hdr-count">{` ${r.count}`}</span> : null}
              {long ? (
                <button
                  type="button"
                  className="reveal"
                  onClick={() =>
                    setRevealed((prev) => {
                      const next = new Set(prev);
                      if (!next.delete(i)) next.add(i);
                      return next;
                    })
                  }
                >
                  {revealed.has(i) ? "hide" : "show"}
                </button>
              ) : null}
            </dd>
          </Fragment>
        );
      })}
    </dl>
  );
}

function safeName(id: string): string {
  return id.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 40) || "exchange";
}

/** Download bytes as a file. Typed as octet-stream so nothing renders it. */
function saveBytes(bytes: Uint8Array<ArrayBuffer>, filename: string) {
  const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
