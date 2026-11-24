// Request/response body rendering. Every value is attacker-controlled and is
// rendered as JSX text, never as HTML.

import { useEffect, useMemo, useState } from "react";
import {
  base64Bytes,
  bodyText,
  headerValue,
  hexRows,
  HEX_BYTES_PER_ROW,
  HEX_ROWS_PER_PAGE,
  humanSize,
  jsonView,
  MAX_BODY_CHARS,
  sha256Hex,
} from "../lib/format.ts";
import type { BodyEncoding, Header } from "../types.ts";

/** The hex view opens on the first 32 bytes, as in the verdigris mockup. */
export const HEX_FIRST_ROWS = 4;

type View = "json" | "raw" | "binary" | "text";

/**
 * A body, with a choice of view: a text body as JSON (pretty-printed, when
 * it parses) or raw (exactly as sent); a binary one as binary (type, size,
 * hash, hex) or as text (decoded as UTF-8, bad bytes as U+FFFD). A view
 * that doesn't apply to a body falls back to its default. Every view is
 * plain text.
 */
export function Body({
  body,
  encoding,
  headers,
  onSave,
}: {
  body: string | null;
  encoding: BodyEncoding;
  headers: Header[];
  onSave?: () => void;
}) {
  const [picked, setPicked] = useState<View | null>(null);
  const binary = encoding === "base64";
  const json = useMemo(() => (body && !binary ? jsonView(body) : null), [body, binary]);
  if (body === null || body === "") return <pre className="none">(empty)</pre>;
  const contentType = headerValue(headers, "content-type");
  const views: { view: View; ok: boolean }[] = binary
    ? [
        { view: "binary", ok: true },
        { view: "text", ok: true },
      ]
    : [
        { view: "json", ok: json !== null },
        { view: "raw", ok: true },
      ];
  const fallback: View = binary ? "binary" : json !== null ? "json" : "raw";
  const view = views.some((v) => v.view === picked && v.ok) ? picked! : fallback;
  const switcher = (
    <div className="body-views" role="group" aria-label="Body view">
      {views.map(({ view: v, ok }) => (
        <button
          key={v}
          type="button"
          aria-pressed={view === v}
          disabled={!ok}
          title={ok ? undefined : "not a JSON object or array"}
          onClick={() => setPicked(v)}
        >
          {v}
        </button>
      ))}
      {!binary && json === null && contentType && /json/i.test(contentType) ? <span className="note">{"content-type says JSON, but it doesn't parse"}</span> : null}
    </div>
  );
  if (view === "binary") {
    return (
      <>
        {switcher}
        <BinaryBody b64={body} contentType={contentType} onSave={onSave} />
      </>
    );
  }
  const text = view === "json" ? json! : view === "text" ? (bodyText(body) ?? "(could not decode base64)") : body;
  const truncated = text.length > MAX_BODY_CHARS;
  return (
    <>
      {switcher}
      <pre data-view={view}>
        {truncated ? text.slice(0, MAX_BODY_CHARS) : text}
        {truncated ? `\n\n… truncated for display (${text.length} chars total)` : null}
      </pre>
    </>
  );
}

/** Binary bodies are never rendered as text: type, size, sha256, optional hex. */
function BinaryBody({ b64, contentType, onSave }: { b64: string; contentType: string | null; onSave?: () => void }) {
  const bytes = useMemo(() => {
    try {
      return base64Bytes(b64);
    } catch {
      return null;
    }
  }, [b64]);
  const [hash, setHash] = useState<string | null>(null);
  const [rows, setRows] = useState(0);
  // WebCrypto exists only in secure contexts (localhost counts).
  const canHash = Boolean(globalThis.crypto?.subtle);

  useEffect(() => {
    if (!bytes || !canHash) return;
    let live = true;
    sha256Hex(bytes).then(
      (h) => live && setHash(`sha256 ${h}`),
      () => live && setHash("sha256 unavailable"),
    );
    return () => {
      live = false;
    };
  }, [bytes, canHash]);

  if (!bytes) return <div className="binary">binary body (could not decode base64)</div>;
  const shownBytes = Math.min(bytes.length, rows * HEX_BYTES_PER_ROW);
  return (
    <div className="binary">
      <div className="binrow">
        <span className="tag">BINARY</span>{" "}
        <span>
          {`${contentType ?? "unknown type"} · `}
          <b>{humanSize(bytes.length)}</b>
          {` · ${bytes.length.toLocaleString("en-US")} bytes`}
        </span>
      </div>
      <div className={hash ? "sha" : "sha none"}>
        {canHash ? (hash ?? "sha256 …") : "sha256 unavailable (needs a secure context)"}
      </div>
      <div className="actions">
        <button type="button" data-action="hex" onClick={() => setRows((r) => (r ? 0 : HEX_FIRST_ROWS))}>
          {rows ? "Hide hex" : "Show hex"} <kbd>x</kbd>
        </button>
        {onSave ? (
          <button type="button" onClick={onSave}>
            Save…
          </button>
        ) : null}
      </div>
      {rows ? (
        <>
          <pre className="hex-dump">{hexRows(bytes, 0, rows)}</pre>
          <p className="more">
            {`first ${shownBytes.toLocaleString("en-US")} of ${bytes.length.toLocaleString("en-US")} bytes`}
            {shownBytes < bytes.length ? (
              <>
                {" · "}
                <button type="button" className="link" onClick={() => setRows((r) => r + HEX_ROWS_PER_PAGE)}>
                  load more
                </button>
              </>
            ) : null}
          </p>
        </>
      ) : null}
    </div>
  );
}
