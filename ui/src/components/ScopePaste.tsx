// Paste a scope list (#38): spall's scope engine previews every line,
// then one audited change applies what's valid. The preview table is
// shared with the OpenAPI import (#40). Pasted text and everything the
// preview echoes back are rendered as text.

import { useEffect, useState } from "react";
import { apiJson, asPreview, type Preview } from "../lib/engagement.ts";

export function PreviewTable({ preview }: { preview: Preview }) {
  if (preview.items.length === 0) return <p className="empty">Nothing to read yet.</p>;
  return (
    <div className="preview-wrap">
      <table className="preview" aria-label="Preview">
        <thead>
          <tr>
            <th scope="col">line</th>
            <th scope="col">you wrote</th>
            <th scope="col">becomes</th>
            <th scope="col">notes</th>
          </tr>
        </thead>
        <tbody>
          {preview.items.map((i, n) => (
            <tr key={n} className={i.error ? "bad" : i.present ? "same" : i.exclude ? "ex" : "ok"}>
              <td className="num">{i.line}</td>
              <td className="src">{i.source}</td>
              <td className="to">{i.error ? `✗ ${i.error}` : `${i.exclude ? "exclude " : "add "}${i.entry ?? ""}`}</td>
              <td className="notes">{i.notes.join("; ")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** How many of each, for the apply button and the summary line. */
export function counts(p: Preview): string {
  const bad = p.items.filter((i) => i.error).length;
  const same = p.items.filter((i) => !i.error && i.present).length;
  const parts = [`${p.add.length} to add`, `${p.exclude.length} to exclude`];
  if (same) parts.push(`${same} already there`);
  if (bad) parts.push(`${bad} not understood (skipped)`);
  return parts.join(" · ");
}

export async function applyPreview(p: Preview): Promise<string | null> {
  const res = await apiJson("/api/scope", { method: "POST", body: { op: "apply", add: p.add, exclude: p.exclude } });
  return typeof res === "string" ? res : null;
}

export function ScopePaste({ onApplied, onClose }: { onApplied(): void; onClose(): void }) {
  const [text, setText] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Preview as you type (debounced): the server's scope engine reads it.
  useEffect(() => {
    if (!text.trim()) return;
    let live = true;
    const id = setTimeout(async () => {
      const res = await apiJson("/api/scope/parse", { method: "POST", body: { text } });
      if (!live) return;
      if (typeof res === "string") {
        setErr(res);
        return;
      }
      setErr(null);
      setPreview(asPreview(res));
    }, 250);
    return () => {
      live = false;
      clearTimeout(id);
    };
  }, [text]);

  const shown = text.trim() ? preview : null;
  const apply = async () => {
    if (!shown) return;
    setBusy(true);
    const r = await applyPreview(shown);
    setBusy(false);
    if (r) return setErr(r);
    setText("");
    setPreview(null);
    onApplied();
    onClose();
  };
  const n = shown ? shown.add.length + shown.exclude.length : 0;
  return (
    <div className="paste" role="group" aria-label="Paste a scope list">
      <p className="note">
        One entry per line, or comma-separated: hosts, *.wildcards, IPs, CIDRs, URLs. Prefix ! (or -) to exclude, or put entries under an "Out of scope:" line.
        Bullets, numbering, # comments and (asides) are ignored.
      </p>
      <textarea
        aria-label="Scope list"
        rows={8}
        spellCheck={false}
        value={text}
        onChange={(ev) => setText(ev.target.value)}
        placeholder={"In scope:\n- https://app.acme.test\n- *.api.acme.test\nOut of scope:\n- pay.acme.test"}
      />
      {shown ? (
        <>
          <p className="summary" role="status">
            {counts(shown)}
          </p>
          <PreviewTable preview={shown} />
        </>
      ) : null}
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
      <div className="actions">
        <button type="button" disabled={!n || busy} onClick={() => void apply()}>
          {n ? `apply ${n}` : "apply"}
        </button>
        <button type="button" onClick={onClose}>
          close
        </button>
      </div>
    </div>
  );
}
