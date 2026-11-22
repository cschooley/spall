// The shared right-hand inspector (docs/ux.md: "One inspector"): one
// exchange, request/response tabs, headers, bodies, replay (M2), copy as
// curl, save body. Flag candidate (M4) lands here later, not as a row
// action. Every value is attacker-controlled and rendered as text.

import { Fragment, useState } from "react";
import { Body } from "./Body.tsx";
import { RequestEditor } from "./RequestEditor.tsx";
import { draftOf, type Draft } from "../lib/edit.ts";
import { editSummary } from "../lib/replays.ts";
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
  timeOf,
} from "../lib/format.ts";
import { toCurl } from "../lib/curl.ts";
import { isSecretHeader } from "../lib/secrets.ts";
import type { Header, TrafficEntry } from "../types.ts";

type Tab = "request" | "response" | "replays";

/** This exchange's replays (#21): history plus live, newest first. */
export interface ReplayList {
  items: TrafficEntry[];
  /** false in log-file mode: only replays already loaded are known. */
  complete: boolean;
  error: string | null;
}

/** The replay control's state, owned by the app (the console shares it). */
export interface ReplayControl {
  /** spall can replay (lever API mode). */
  available: boolean;
  busy: boolean;
  /** Set while an unsafe method waits for its second press. */
  armedLabel: string | null;
  /** Why the last attempt didn't send (denied, refused, error). */
  message: string | null;
  /** Replay as captured, or with the editor's draft (#19). */
  onReplay(draft?: Draft): void;
  /** The draft changed or the editor opened: a confirmation no longer applies. */
  onDraftChange(): void;
}

export function Inspector({
  entry,
  onClose,
  replay,
  onOpen,
  replays,
  chain,
}: {
  entry: TrafficEntry;
  onClose: () => void;
  replay: ReplayControl;
  replays: ReplayList;
  /** Ids from the first original to this exchange; `missing` if a link is gone. */
  chain: { chain: string[]; missing: string | null };
  /** Open another exchange by id (the original of a replay). */
  onOpen(id: string): void;
}) {
  const [tab, setTab] = useState<Tab>("request");
  const [draft, setDraftState] = useState<Draft | null>(null);
  const setDraft = (d: Draft | null) => {
    setDraftState(d);
    replay.onDraftChange();
  };
  const startEditing = () => {
    setTab("request");
    setDraft(draftOf(entry));
  };
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
          {entry.replay_of ? (
            <button type="button" className="link orig" title="Open the original" onClick={() => onOpen(entry.replay_of!)}>
              {`replay of ${entry.replay_of}`}
            </button>
          ) : null}
        </div>
        {chain.chain.length > 2 || chain.missing ? (
          <div className="chain" aria-label="Replay chain">
            {"chain: "}
            {chain.missing ? <span title={`${chain.missing} isn't in taphttp's history`}>{"… → "}</span> : null}
            {chain.chain.map((id, i) =>
              id === entry.id ? (
                <span key={id} className="here">
                  this
                </span>
              ) : (
                <Fragment key={id}>
                  <button type="button" className="link" title={id} onClick={() => onOpen(id)}>
                    {i === 0 && !chain.missing ? `${shortId(id)} (original)` : shortId(id)}
                  </button>
                  {" → "}
                </Fragment>
              ),
            )}
          </div>
        ) : null}
        <button type="button" className="close" aria-label="Close inspector" title="Close (Esc)" onClick={onClose}>
          ×
        </button>
      </header>
      {draft ? null : (
        <div className="actions">
          <button
            type="button"
            className={replay.armedLabel ? "replay armed" : "replay"}
            data-action="replay"
            disabled={!replay.available || replay.busy}
            title={replay.available ? "Replay as captured (⏎)" : "Replay needs taphttp's lever API"}
            onClick={() => replay.onReplay()}
          >
            {replay.busy ? "Sending…" : (replay.armedLabel ?? "Replay")}
            <kbd>⏎</kbd>
          </button>
          <button type="button" data-action="edit" disabled={!replay.available} title="Edit, then replay (e)" onClick={startEditing}>
            Edit & replay <kbd>e</kbd>
          </button>
          <button type="button" onClick={copyCurl}>
            {copied === "ok" ? "copied" : copied === "failed" ? "copy failed" : "Copy as curl"}
          </button>
          <button type="button" disabled={!bytes} onClick={save}>
            {`Save ${tab} body`}
          </button>
          {replay.message ? (
            <p className="replay-msg" role="status">
              {replay.message}
            </p>
          ) : null}
        </div>
      )}
      <div className="tabs" role="tablist">
        {(["request", "response", "replays"] as const).map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} className={tab === t ? "on" : undefined} onClick={() => setTab(t)}>
            {t}
            {t === "replays" ? <span className="count">{replays.items.length}</span> : null}
          </button>
        ))}
      </div>
      <section className="pane" role="tabpanel" aria-label={tab}>
        {tab === "replays" ? (
          <Replays entry={entry} list={replays} onOpen={onOpen} />
        ) : draft && isReq ? (
          <RequestEditor original={entry} draft={draft} setDraft={setDraft} replay={replay} onCancel={() => setDraft(null)} />
        ) : (
          <>
            <h3>
              Headers <span className="note">· wire order, repeats kept</span>
            </h3>
            <Headers headers={headers} />
            <h3>
              Body
              {bytes ? <span className="note">{` · ${type ?? "no content-type"} · ${humanSize(bytes.length)}`}</span> : null}
            </h3>
            <Body body={body} encoding={encoding} headers={headers} onSave={save} />
          </>
        )}
      </section>
    </aside>
  );
}

function Replays({ entry, list, onOpen }: { entry: TrafficEntry; list: ReplayList; onOpen(id: string): void }) {
  return (
    <>
      <h3>
        {`Replays of this exchange`}
        <span className="note">{list.complete ? " · newest first" : " · loaded replays only (log-file mode: no history API)"}</span>
      </h3>
      {list.error ? <p className="replay-msg">{list.error}</p> : null}
      {list.items.length === 0 ? (
        <p className="none">No replays yet. ⏎ replays this exchange; e edits first.</p>
      ) : (
        <ol className="replays">
          {list.items.map((r) => (
            <li key={r.id}>
              <button type="button" className="link" title={`Open ${r.id}`} onClick={() => onOpen(r.id)}>
                <span className="t">{timeOf(r.ts)}</span>
                <span className={statusClass(r.status)}>{statusLine(r.status)}</span>
                <span>{r.duration_ms !== null ? durationText(r.duration_ms) : "—"}</span>
                <span className="what">{editSummary(entry, r)}</span>
                {r.meta?.actor ? <span className="who">{r.meta.actor}</span> : null}
              </button>
              {r.error ? <span className="error">{r.error}</span> : null}
            </li>
          ))}
        </ol>
      )}
    </>
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
