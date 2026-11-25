// The Surface view (#95, #94; docs/ux.md): every endpoint seen in traffic
// or documented in an imported API description, ids folded into a
// template, with who touched it and its coverage by WSTG category. The
// strip on top counts what's untested per category and filters to it; the
// pane shows one endpoint: its params, an example exchange, each
// category's state with who and how, and a way to mark it tested yourself.
// Hosts, templates and params came from captured traffic: text only.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AgentVerbs } from "./AgentVerbs.tsx";
import { timeOf } from "../lib/format.ts";
import { filterRows, loadSurface, mark, markCoverage, totals, type SurfaceListing, type SurfaceRow } from "../lib/surface.ts";

const CELL_WORD = { found: "found", tested: "tested" } as const;

export function SurfaceView({ available, onOpenExchange }: { available: boolean; onOpenExchange(id: string): void }) {
  const [data, setData] = useState<SurfaceListing | string | null>(null);
  const [all, setAll] = useState(false);
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<string | null>(null);
  const filterRef = useRef<HTMLInputElement>(null);

  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  useEffect(() => {
    let live = true;
    const poll = async () => {
      const d = await loadSurface(all);
      if (live) setData(d);
    };
    void poll();
    const t = setInterval(() => void poll(), 5000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [all, nonce]);

  const listing = typeof data === "object" && data !== null ? data : null;
  const cats = useMemo(() => listing?.categories ?? [], [listing]);
  const shown = useMemo(() => (listing ? filterRows(listing.rows, query) : []), [listing, query]);
  const sums = useMemo(() => (listing ? totals(listing.rows, cats) : {}), [listing, cats]);
  const current = shown.find((r) => r.key === sel) ?? shown[0] ?? null;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "/") {
        e.preventDefault();
        filterRef.current?.focus();
      } else if (e.key === "j" || e.key === "k") {
        const i = shown.findIndex((r) => r.key === current?.key);
        const next = shown[Math.max(0, Math.min(shown.length - 1, i + (e.key === "j" ? 1 : -1)))];
        if (next) setSel(next.key);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shown, current]);

  const toggleUntested = (code: string) => {
    const word = `untested:${code}`;
    const words = query.split(/\s+/).filter(Boolean);
    setQuery(words.includes(word) ? words.filter((w) => w !== word).join(" ") : [...words.filter((w) => !/^untested:/i.test(w)), word].join(" "));
  };

  if (!available)
    return (
      <section className="surface" aria-label="Surface">
        <p className="empty">The Surface needs spall's lever API (it isn't available in log mode).</p>
      </section>
    );

  return (
    <section className="surface" aria-label="Surface">
      <div className="queue">
        <div className="toolbar">
          <input
            ref={filterRef}
            type="search"
            className="filter"
            aria-label="Filter endpoints"
            placeholder="filter   untested:ATHZ  found  host:shop  method:POST  documented  agent  text"
            value={query}
            onChange={(ev) => setQuery(ev.target.value)}
            onKeyDown={(ev) => ev.key === "Escape" && ev.currentTarget.blur()}
          />
          <label className="check">
            <input type="checkbox" checked={all} onChange={(ev) => setAll(ev.target.checked)} />
            out of scope too
          </label>
        </div>
        {cats.length ? (
          <div className="cov-strip" role="group" aria-label="Untested by category">
            {cats.map((c) => {
              const s = sums[c.code];
              const on = query.split(/\s+/).includes(`untested:${c.code}`);
              return (
                <button key={c.code} type="button" className={on ? "on" : undefined} aria-pressed={on} title={`${c.name}: ${s?.found ?? 0} found, ${s?.tested ?? 0} tested, ${s?.untested ?? 0} untested. Show the untested ones.`} onClick={() => toggleUntested(c.code)}>
                  <span className="code">{c.code}</span>
                  <span className="n">{s?.untested ?? 0}</span>
                </button>
              );
            })}
          </div>
        ) : null}
        {typeof data === "string" ? (
          <p className="error" role="alert">
            {data}
          </p>
        ) : null}
        {data === null ? <p className="empty">loading…</p> : null}
        {listing && shown.length === 0 ? (
          <p className="empty">
            {listing.rows.length === 0
              ? all
                ? "No endpoints yet. Browse the app through the proxy, or import an OpenAPI description in the Engagement view."
                : "No in-scope endpoints yet. Add the app's hosts to scope in the Engagement view, or tick “out of scope too”."
              : "Nothing here matches."}
          </p>
        ) : null}
        {shown.length ? (
          <div className="surf-table" role="table" aria-label="Endpoints">
            <div className="surf-row head" role="row">
              <span role="columnheader">method</span>
              <span role="columnheader">endpoint</span>
              <span role="columnheader" className="num">hits</span>
              <span role="columnheader">who</span>
              <span role="columnheader" className="marks" title={cats.map((c) => `${c.code} ${c.name}`).join("\n")}>
                {cats.map((c) => c.code.slice(0, 2)).join(" ")}
              </span>
            </div>
            {shown.map((r) => (
              <button
                key={r.key}
                type="button"
                role="row"
                className={`surf-row${r.key === current?.key ? " sel" : ""}${r.scope !== "in" ? " oos" : ""}`}
                aria-current={r.key === current?.key ? "true" : undefined}
                onClick={() => setSel(r.key)}
              >
                <span role="cell" className="method">{r.method}</span>
                <span role="cell" className="where">
                  <span className="host">{r.host}</span>
                  {r.template}
                </span>
                <span role="cell" className="num">{r.count || "—"}</span>
                <span role="cell" className="who">{who(r)}</span>
                <span role="cell" className="marks">
                  {cats.map((c) => {
                    const cell = r.coverage[c.code];
                    return (
                      <span key={c.code} className={`m ${cell?.state ?? "untested"}`} title={`${c.code}: ${cell?.state ?? "untested"}`}>
                        {mark(cell)}
                      </span>
                    );
                  })}
                </span>
              </button>
            ))}
          </div>
        ) : null}
        <p className="note keys">
          {listing ? `${shown.length} of ${listing.rows.length} endpoints · ` : ""}! found · ✓ tested · · untested · j/k move · / filter
          {listing?.dropped ? ` · ${listing.dropped} not tracked (limit reached)` : ""}
        </p>
      </div>

      {current ? <Pane key={current.key} row={current} categories={cats} available={available} onOpenExchange={onOpenExchange} onMarked={reload} /> : null}
    </section>
  );
}

function who(r: SurfaceRow): string {
  return [r.byYou ? "you" : "", ...r.tasks.map((t) => `◆ ${t}`), r.documented ? "documented" : ""].filter(Boolean).join(" · ") || "—";
}

function Pane({
  row,
  categories,
  available,
  onOpenExchange,
  onMarked,
}: {
  row: SurfaceRow;
  categories: readonly { code: string; name: string }[];
  available: boolean;
  onOpenExchange(id: string): void;
  onMarked(): void;
}) {
  const firstUntested = categories.find((c) => !row.coverage[c.code])?.code ?? categories[0]?.code ?? "INFO";
  const [cat, setCat] = useState(firstUntested);
  const [note, setNote] = useState("");
  const [msg, setMsg] = useState<{ text: string; bad: boolean } | null>(null);

  const submit = async () => {
    const why = await markCoverage(row.key, cat, note.trim());
    if (why) return setMsg({ text: `mark: ${why}`, bad: true });
    setMsg({ text: `marked ${cat} tested`, bad: false });
    setNote("");
    onMarked();
  };

  return (
    <aside className="inspector surf" aria-label="Endpoint">
      <header>
        <h2>
          {`${row.method} `}
          <span className="host">{row.host}</span>
          {row.template}
        </h2>
        <div className="meta">
          <span>{`scope: ${row.scope}`}</span>
          <span>{row.count ? `${row.count} seen` : "not seen yet"}</span>
          {row.statuses.length ? <span>{`status ${row.statuses.join(", ")}`}</span> : null}
          {row.documented ? <span>documented</span> : null}
          {row.lastSeen ? <span>{`last ${timeOf(row.lastSeen)}`}</span> : null}
        </div>
      </header>
      <section className="pane">
        <dl className="kv">
          <dt>who</dt>
          <dd>{who(row)}</dd>
          <dt>params</dt>
          <dd>{row.params.length ? row.params.join(", ") : "—"}</dd>
          <dt>example</dt>
          <dd>
            {row.example ? (
              <button type="button" className="link" onClick={() => onOpenExchange(row.example ?? "")}>
                {row.example}
              </button>
            ) : (
              "—"
            )}
          </dd>
        </dl>
        <h3>Coverage</h3>
        <ul className="cov-list" aria-label="Coverage by category">
          {categories.map((c) => {
            const cell = row.coverage[c.code];
            return (
              <li key={c.code} className={cell?.state ?? "untested"}>
                <span className="m">{mark(cell)}</span>
                <span className="code">{c.code}</span>
                <span className="name">{c.name}</span>
                <span className="state">{cell ? CELL_WORD[cell.state] : "untested"}</span>
                <span className="detail">
                  {cell?.candidates.map((id) => (
                    <a key={id} href={`#findings/${encodeURIComponent(id)}`}>
                      {id}
                    </a>
                  ))}
                  {cell && cell.state === "tested" ? cell.by.map((b, i) => `${b.startsWith("agent:") ? "◆ " + b.slice(6) : b}${cell.how[i] ? ` (${cell.how[i]})` : ""}`).join("; ") : null}
                </span>
              </li>
            );
          })}
        </ul>
        <form
          className="mark-form"
          aria-label="Mark tested"
          onSubmit={(ev) => {
            ev.preventDefault();
            void submit();
          }}
        >
          <select aria-label="Category tested" value={cat} onChange={(ev) => setCat(ev.target.value)}>
            {categories.map((c) => (
              <option key={c.code} value={c.code}>{`${c.code} ${c.name}`}</option>
            ))}
          </select>
          <input type="text" aria-label="What you tested" placeholder="what you tested (optional)" maxLength={500} value={note} onChange={(ev) => setNote(ev.target.value)} />
          <button type="submit">Mark tested</button>
        </form>
        {msg ? (
          <p className={msg.bad ? "replay-msg" : "ok-msg"} role="status">
            {msg.text}
          </p>
        ) : null}
        {row.example ? <AgentVerbs key={`verbs-${row.example}`} on="exchange" id={row.example} available={available} /> : <p className="note wrap">No exchange on it yet: the agent verbs need one. Browse to it through the proxy first.</p>}
      </section>
    </aside>
  );
}
