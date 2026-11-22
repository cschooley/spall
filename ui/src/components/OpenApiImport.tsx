// Import an OpenAPI or Swagger file (#40). spall reads it server-side (never
// fetching anything it references) and previews its servers like a pasted
// scope list; one click adds the hosts and keeps the spec's paths on the
// engagement for Surface. Everything shown is text from an untrusted file.

import { useState, type ChangeEvent } from "react";
import { apiJson, asImport, type ApiSpec, type Preview } from "../lib/engagement.ts";
import { applyPreview, counts, PreviewTable } from "./ScopePaste.tsx";

const MAX_BYTES = 4 * 1024 * 1024;

export function OpenApiImport({ kept, onApplied, onClose }: { kept: ApiSpec[]; onApplied(): void; onClose(): void }) {
  const [read, setRead] = useState<{ name: string; spec: ApiSpec; notes: string[]; preview: Preview } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const choose = async (ev: ChangeEvent<HTMLInputElement>) => {
    const file = ev.target.files?.[0];
    setRead(null);
    if (!file) return;
    if (file.size > MAX_BYTES) return setErr(`${file.name}: at most 4 MiB`);
    setBusy(true);
    const res = await apiJson("/api/openapi/parse", { method: "POST", body: { text: await file.text() } });
    setBusy(false);
    if (typeof res === "string") return setErr(`${file.name}: ${res}`);
    const i = asImport(res);
    if (!i) return setErr("unexpected answer from spall");
    setErr(null);
    setRead({ name: file.name, ...i });
  };

  const apply = async () => {
    if (!read) return;
    setBusy(true);
    const n = read.preview.add.length + read.preview.exclude.length;
    const scopeErr = n ? await applyPreview(read.preview) : null;
    const keepErr = scopeErr
      ? null
      : await apiJson("/api/engagement", { method: "PATCH", body: { api_specs: [...kept, read.spec] } }).then((r) => (typeof r === "string" ? r : null));
    setBusy(false);
    if (scopeErr || keepErr) return setErr(scopeErr ?? keepErr);
    onApplied();
    onClose();
  };

  const n = read ? read.preview.add.length + read.preview.exclude.length : 0;
  return (
    <div className="paste" role="group" aria-label="Import an OpenAPI file">
      <p className="note">
        An OpenAPI 3 or Swagger 2 file, JSON or YAML. Its servers become proposed scope entries; nothing it references is fetched. Its paths are kept on the
        engagement for Surface.
      </p>
      <input type="file" aria-label="OpenAPI file" accept=".json,.yaml,.yml,application/json,application/yaml,text/yaml" onChange={(ev) => void choose(ev)} />
      {read ? (
        <>
          <p className="summary" role="status">
            {`${read.spec.title}${read.spec.version ? ` ${read.spec.version}` : ""} (${read.spec.format}) · ${read.spec.paths.length} paths · ${counts(read.preview)}`}
          </p>
          {read.notes.length ? (
            <ul className="notes" aria-label="Import notes">
              {read.notes.map((x, i) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          ) : null}
          <PreviewTable preview={read.preview} />
        </>
      ) : null}
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
      <div className="actions">
        <button type="button" disabled={!read || busy} onClick={() => void apply()}>
          {n ? `apply ${n} and keep the spec` : "keep the spec"}
        </button>
        <button type="button" onClick={onClose}>
          close
        </button>
      </div>
    </div>
  );
}
