// Request/response body rendering. Every value is attacker-controlled and is
// rendered as JSX text, never as HTML.

import { useEffect, useMemo, useState } from "react";
import {
  base64Bytes,
  headerValue,
  hexRows,
  HEX_BYTES_PER_ROW,
  HEX_ROWS_PER_PAGE,
  humanSize,
  prettyBody,
  sha256Hex,
} from "../lib/format.ts";
import type { BodyEncoding, Header } from "../types.ts";

/** The hex view opens on the first 32 bytes, as in the verdigris mockup. */
export const HEX_FIRST_ROWS = 4;

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
  if (body === null || body === "") return <pre className="none">(empty)</pre>;
  if (encoding === "base64")
    return <BinaryBody b64={body} contentType={headerValue(headers, "content-type")} onSave={onSave} />;
  const { text, truncated } = prettyBody(body, headerValue(headers, "content-type"));
  return (
    <pre>
      {text}
      {truncated ? `\n\n… truncated for display (${body.length} chars total)` : null}
    </pre>
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
