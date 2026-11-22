// Edit & replay (#19): the captured request as editable fields, changes
// marked against the original, and the limits stated rather than ignored.
// Every value is attacker-controlled: shown in inputs (as text), sent as
// data, never rendered as HTML.

import { changes, hostMismatch, replayBodyFor, scatteredRepeats, type Draft } from "../lib/edit.ts";
import { bodyBytes, humanSize } from "../lib/format.ts";
import type { Header, TrafficEntry } from "../types.ts";
import type { ReplayControl } from "./Inspector.tsx";
import { modKey } from "./ShortcutSheet.tsx";

export function RequestEditor({
  original,
  draft,
  setDraft,
  replay,
  onCancel,
}: {
  original: TrafficEntry;
  draft: Draft;
  setDraft: (d: Draft) => void;
  replay: ReplayControl;
  onCancel: () => void;
}) {
  const c = changes(original, draft);
  const built = replayBodyFor(original, draft);
  const mismatch = hostMismatch(original.url, draft.headers);
  const urlHost = hostOfUrl(original.url) ?? original.host;
  const setHeaders = (headers: Header[]) => setDraft({ ...draft, headers });
  const setHeader = (i: number, h: Header) => setHeaders(draft.headers.map((x, j) => (j === i ? h : x)));
  const move = (i: number, d: number) => {
    const j = i + d;
    if (j < 0 || j >= draft.headers.length) return;
    const next = [...draft.headers];
    [next[i], next[j]] = [next[j]!, next[i]!];
    setHeaders(next);
  };
  const send = () => {
    if (built.ok) replay.onReplay(draft);
  };

  return (
    <div
      className="editor"
      role="form"
      aria-label="Edit request"
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
          e.preventDefault();
          send();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onCancel();
        }
      }}
    >
      <p className="note">
        {`Edit & replay · ${c.count === 0 ? "no changes: replays as captured" : `${c.count} change${c.count === 1 ? "" : "s"}`}`}
        {" · the URL is fixed: replay goes to the original target"}
      </p>

      <h3>Method</h3>
      <input
        className={c.method ? "method changed" : "method"}
        aria-label="Method"
        value={draft.method}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setDraft({ ...draft, method: e.target.value.toUpperCase() })}
      />

      <h3>
        Headers <span className="note">· sent in this order</span>
      </h3>
      <div className="hedit">
        {draft.headers.map(([name, value], i) => (
          <div key={i} className={c.headers[i] ? "hrow changed" : "hrow"}>
            <input aria-label={`Header ${i + 1} name`} value={name} spellCheck={false} autoComplete="off" onChange={(e) => setHeader(i, [e.target.value, value])} />
            <input aria-label={`Header ${i + 1} value`} value={value} spellCheck={false} autoComplete="off" onChange={(e) => setHeader(i, [name, e.target.value])} />
            <button type="button" aria-label={`Move header ${i + 1} up`} title="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
              ↑
            </button>
            <button type="button" aria-label={`Move header ${i + 1} down`} title="Move down" disabled={i === draft.headers.length - 1} onClick={() => move(i, 1)}>
              ↓
            </button>
            <button type="button" aria-label={`Remove header ${i + 1}`} title="Remove" onClick={() => setHeaders(draft.headers.filter((_, j) => j !== i))}>
              ×
            </button>
          </div>
        ))}
        <button type="button" className="add" onClick={() => setHeaders([...draft.headers, ["", ""]])}>
          + Add header
        </button>
      </div>
      {c.removed ? <p className="note">{`${c.removed} captured header${c.removed === 1 ? "" : "s"} removed`}</p> : null}
      {mismatch !== null ? (
        <p className="caution" role="note">
          {`Host is ${mismatch}, not the URL's host ${urlHost}. That can reach a different site on the same server; both must be in scope.`}
        </p>
      ) : null}
      {scatteredRepeats(draft.headers) ? (
        <p className="note">Repeated headers split by others are sent together, at the first one's position.</p>
      ) : null}

      <h3>Body</h3>
      {draft.body === null ? (
        <p className="note">{`Binary body (${humanSize(bodyBytes(original.req_body, original.req_body_encoding)?.length ?? 0)}): sent as captured, not editable here.`}</p>
      ) : (
        <textarea
          className={c.body ? "changed" : undefined}
          aria-label="Body"
          rows={8}
          spellCheck={false}
          value={draft.body}
          onChange={(e) => setDraft({ ...draft, body: e.target.value })}
        />
      )}
      {built.ok && built.contentLength !== null ? <p className="note">{`Content-Length will be sent as ${built.contentLength}.`}</p> : null}
      {!built.ok ? (
        <p className="replay-msg" role="alert">
          {built.error}
        </p>
      ) : null}
      {replay.message ? (
        <p className="replay-msg" role="status">
          {replay.message}
        </p>
      ) : null}

      <div className="actions">
        <button
          type="button"
          className={replay.armedLabel ? "replay armed" : "replay"}
          data-action="replay"
          disabled={!replay.available || replay.busy || !built.ok}
          title={replay.available ? `Send (${modKey()} ⏎)` : "Replay needs taphttp's lever API"}
          onClick={send}
        >
          {replay.busy ? "Sending…" : (replay.armedLabel ?? "Send")}
          <kbd>{`${modKey()} ⏎`}</kbd>
        </button>
        <button type="button" onClick={onCancel}>
          Cancel <kbd>Esc</kbd>
        </button>
      </div>
    </div>
  );
}

function hostOfUrl(url: string): string | null {
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}
