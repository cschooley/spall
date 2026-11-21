// Expanded view of one exchange. Every value is attacker-controlled and is
// rendered as JSX text, never as HTML.

import { Fragment, useEffect, useMemo, useState } from "react";
import {
  base64Bytes,
  headerRows,
  headerValue,
  hexRows,
  HEX_BYTES_PER_ROW,
  HEX_ROWS_PER_PAGE,
  prettyBody,
  sha256Hex,
} from "../lib/format.ts";
import type { BodyEncoding, Header, TrafficEntry } from "../types.ts";

export function Detail({ entry }: { entry: TrafficEntry }) {
  return (
    <>
      <div className="meta">
        <b>{entry.method} </b>
        {entry.url}
        <br />
        {`id ${entry.id}`}
        {entry.replay_of ? ` · replay of ${entry.replay_of}` : null}
      </div>
      <div className="detail-grid">
        <section>
          <h3>request headers</h3>
          <Headers headers={entry.req_headers} />
          <h3>request body</h3>
          <Body body={entry.req_body} encoding={entry.req_body_encoding} headers={entry.req_headers} />
        </section>
        <section>
          <h3>response headers</h3>
          <Headers headers={entry.res_headers} />
          <h3>response body</h3>
          <Body body={entry.res_body} encoding={entry.res_body_encoding} headers={entry.res_headers} />
        </section>
      </div>
    </>
  );
}

function Headers({ headers }: { headers: Header[] }) {
  const rows = headerRows(headers);
  if (rows.length === 0) return <pre className="none">(none)</pre>;
  return (
    <pre>
      {rows.map((r, i) => (
        <Fragment key={i}>
          {i > 0 ? "\n" : null}
          {`${r.name}: ${r.value}`}
          {r.count ? <span className="hdr-count">{` ${r.count}`}</span> : null}
        </Fragment>
      ))}
    </pre>
  );
}

function Body({
  body,
  encoding,
  headers,
}: {
  body: string | null;
  encoding: BodyEncoding;
  headers: Header[];
}) {
  if (body === null || body === "") return <pre className="none">(empty)</pre>;
  if (encoding === "base64") return <BinaryBody b64={body} contentType={headerValue(headers, "content-type")} />;
  const { text, truncated } = prettyBody(body, headerValue(headers, "content-type"));
  return (
    <pre>
      {text}
      {truncated ? `\n\n… truncated for display (${body.length} chars total)` : null}
    </pre>
  );
}

/** Binary bodies are never rendered as text: type, size, sha256, optional hex. */
function BinaryBody({ b64, contentType }: { b64: string; contentType: string | null }) {
  const bytes = useMemo(() => {
    try {
      return base64Bytes(b64);
    } catch {
      return null;
    }
  }, [b64]);
  const [hash, setHash] = useState<string | null>(null);
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

  if (!bytes) return <pre className="binary">binary body (could not decode base64)</pre>;
  return (
    <div>
      <pre className="binary">
        <span>{`binary · ${contentType ?? "unknown type"} · ${bytes.length} bytes`}</span>
        {"\n"}
        {canHash ? (
          <span className={hash ? undefined : "none"}>{hash ?? "sha256 …"}</span>
        ) : (
          <span className="none">sha256 unavailable (needs a secure context)</span>
        )}
      </pre>
      <HexView bytes={bytes} />
    </div>
  );
}

function HexView({ bytes }: { bytes: Uint8Array }) {
  const [open, setOpen] = useState(false);
  const [rows, setRows] = useState(HEX_ROWS_PER_PAGE);
  const totalRows = Math.ceil(bytes.length / HEX_BYTES_PER_ROW);
  const left = bytes.length - Math.min(rows, totalRows) * HEX_BYTES_PER_ROW;
  return (
    <div className="hex">
      <button type="button" onClick={() => setOpen((o) => !o)}>
        {open ? "hide hex" : "hex view"}
      </button>
      {open ? (
        <>
          <pre className="hex-dump">{hexRows(bytes, 0, rows)}</pre>
          {left > 0 ? (
            <button type="button" onClick={() => setRows((r) => r + HEX_ROWS_PER_PAGE)}>
              {`next ${Math.min(left, HEX_ROWS_PER_PAGE * HEX_BYTES_PER_ROW)} bytes (${left} left)`}
            </button>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
