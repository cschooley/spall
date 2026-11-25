// The Engagement view (#37, docs/ux.md §1): what the test is, its scope and
// its rules, and (later) what's waiting on you. Every change goes through
// the same audited API as the console. Engagement details are typed by the
// tester, but everything here is still rendered as text.

import { loadOob, mintOob, type OobPayload, type OobState } from "../lib/oob.ts";
import { changeChecks, loadChecks, type CheckInfo } from "../lib/sweep.ts";
import { changeNever, loadNever, setException, SUGGESTED, type NeverList } from "../lib/never.ts";
import { By } from "./Attribution.tsx";
import { activityOf, type ActivityLine } from "../lib/attribution.ts";
import { Approvals } from "./Approvals.tsx";
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { ScopePaste } from "./ScopePaste.tsx";
import { OpenApiImport } from "./OpenApiImport.tsx";
import { BrowserSetup } from "./BrowserSetup.tsx";
import type { TrafficEntry } from "../types.ts";
import { parentWildcard, type HostListing, type HostState } from "../lib/hosts.ts";
import { loadSurface, totals, type SurfaceListing } from "../lib/surface.ts";
import { timeOf } from "../lib/format.ts";
import type { EngagementInfo, ScopeVerdict } from "../commands/registry.ts";
import { apiJson, expiryText } from "../lib/engagement.ts";
import { daysText, parseDays, parseSpan, rulesFormOf, rulesPatchOf, type Rules, type RulesForm } from "../lib/roe.ts";
import { useEngagement } from "../useEngagement.ts";
import { listCandidates, stateOf, where, type Candidate } from "../lib/candidates.ts";

type Result = string | null;

/** Run a change; a string result is an error message. */
async function send(path: string, method: string, body: unknown): Promise<Result> {
  const res = await apiJson(path, { method, body });
  return typeof res === "string" ? res : null;
}

export function EngagementView({
  available,
  hosts,
  proxy,
  latestHttps,
  onChanged,
  toTriage,
  waitingApproval,
  onOpenExchange,
}: {
  /** Open an exchange in Traffic's inspector. */
  onOpenExchange(id: string): void;
  /** The status bar's count; a change reloads the list (#47). */
  toTriage: number | null;
  /** Tasks waiting on approval (#56); a change reloads them. */
  waitingApproval: number | null;
  available: boolean;
  hosts: HostListing | null;
  proxy: string | null;
  latestHttps: TrafficEntry | null;
  onChanged(): void;
}) {
  const st = useEngagement(available);
  const done = () => {
    st.refresh();
    onChanged();
  };
  if (!available) {
    return (
      <div className="eng">
        <p className="empty">Engagements need taphttp's lever API (start spall with --taphttp-api).</p>
      </div>
    );
  }
  if (!st.engagement) {
    return <div className="eng">{st.loading ? <p className="empty">loading…</p> : <p className="error" role="alert">{st.error}</p>}</div>;
  }
  const e = st.engagement;
  return (
    <div className="eng">
      <Header e={e} today={st.today} list={st.list} onChanged={done} />
      <div className="eng-grid">
        <Details e={e} onChanged={done} />
        <ScopePanel e={e} onChanged={done} />
        <NeverPanel key={e.id} onChanged={done} />
        <ChecksPanel key={`checks-${e.id}`} onChanged={done} />
        <OobPanel key={`oob-${e.id}`} onOpenExchange={onOpenExchange} />
        <HostsPanel listing={hosts} onChanged={done} />
        <RulesPanel e={e} onChanged={done} />
        <Waiting toTriage={toTriage} waitingApproval={waitingApproval} onChanged={onChanged} />
        <SetupSection proxy={proxy} latestHttps={latestHttps} />
      </div>
    </div>
  );
}

function Section({ title, label, actions, children }: { title: string; label?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="sect" aria-label={label ?? title}>
      <h3>
        {title}
        {actions ? <span className="sect-actions">{actions}</span> : null}
      </h3>
      {children}
    </section>
  );
}

function ErrorLine({ msg }: { msg: Result }) {
  return msg ? (
    <p className="error" role="alert">
      {msg}
    </p>
  ) : null;
}

function Header({
  e,
  today,
  list,
  onChanged,
}: {
  e: EngagementInfo;
  today: string | null;
  list: { id: string; name: string; active: boolean }[];
  onChanged(): void;
}) {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [err, setErr] = useState<Result>(null);
  const expiry = today ? expiryText(e.expires, today) : null;
  const now = e.testingNow;
  const switchTo = async (id: string) => {
    const r = await send("/api/engagements/active", "POST", { id });
    setErr(r);
    if (!r) onChanged();
  };
  const create = async (ev: FormEvent) => {
    ev.preventDefault();
    const r = await send("/api/engagements", "POST", { name });
    setErr(r);
    if (!r) {
      setCreating(false);
      setName("");
      onChanged();
    }
  };
  return (
    <header className="eng-head">
      <div className="eng-title">
        <h2 className="title">{e.name}</h2>
        <div className="meta">
          {e.client ? <span>{e.client}</span> : null}
          <span title="Engagement id">{e.id}</span>
          <span>{e.starts || e.ends ? `${e.starts ?? "?"} → ${e.ends ?? "?"}` : "dates not set"}</span>
          {expiry ? <span className={`expiry ${expiry.tone}`}>{expiry.text}</span> : <span>authorization expiry not set</span>}
        </div>
        {now ? (
          <p className={`chip testing ${now.allowed ? "allowed" : "closed"}`} title={now.reason ?? "The rules of engagement allow testing now."}>
            {now.allowed ? "testing allowed now" : `not testing now: ${now.reason ?? ""}`}
          </p>
        ) : null}
      </div>
      <div className="eng-switch">
        {list.length > 1 ? (
          <label>
            {"engagement "}
            <select aria-label="Active engagement" value={e.id} onChange={(ev) => void switchTo(ev.target.value)}>
              {list.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.name}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        {creating ? (
          <form className="inline" onSubmit={(ev) => void create(ev)}>
            <input aria-label="New engagement name" placeholder="name" value={name} onChange={(ev) => setName(ev.target.value)} autoFocus />
            <button type="submit" disabled={!name.trim()}>
              create
            </button>
            <button type="button" onClick={() => setCreating(false)}>
              cancel
            </button>
          </form>
        ) : (
          <button type="button" onClick={() => setCreating(true)}>
            new engagement
          </button>
        )}
        <ErrorLine msg={err} />
      </div>
    </header>
  );
}

const FIELDS = [
  ["name", "Name"],
  ["client", "Client"],
  ["starts", "Starts"],
  ["ends", "Ends"],
  ["expires", "Authorization expires"],
] as const;

function Details({ e, onChanged }: { e: EngagementInfo; onChanged(): void }) {
  const [form, setForm] = useState<Record<string, string> | null>(null);
  const [err, setErr] = useState<Result>(null);
  const start = () =>
    setForm({ name: e.name, client: e.client, starts: e.starts ?? "", ends: e.ends ?? "", expires: e.expires ?? "", notes: e.notes });
  const save = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!form) return;
    const current: Record<string, string> = { name: e.name, client: e.client, starts: e.starts ?? "", ends: e.ends ?? "", expires: e.expires ?? "", notes: e.notes };
    const patch = Object.fromEntries(Object.entries(form).filter(([k, v]) => v !== current[k]));
    const r = Object.keys(patch).length ? await send("/api/engagement", "PATCH", patch) : null;
    setErr(r);
    if (!r) {
      setForm(null);
      onChanged();
    }
  };
  return (
    <Section
      title="Engagement"
      label="Engagement details"
      actions={
        form ? null : (
          <button type="button" onClick={start}>
            edit
          </button>
        )
      }
    >
      {form ? (
        <form className="eng-form" onSubmit={(ev) => void save(ev)}>
          {FIELDS.map(([k, label]) => (
            <label key={k}>
              <span>{label}</span>
              <input
                type={k === "name" || k === "client" ? "text" : "date"}
                value={form[k]}
                onChange={(ev) => setForm({ ...form, [k]: ev.target.value })}
                required={k === "name"}
              />
            </label>
          ))}
          <label>
            <span>Notes</span>
            <textarea rows={4} value={form.notes} onChange={(ev) => setForm({ ...form, notes: ev.target.value })} />
          </label>
          <div className="actions">
            <button type="submit">save</button>
            <button type="button" onClick={() => setForm(null)}>
              cancel
            </button>
          </div>
          <ErrorLine msg={err} />
        </form>
      ) : (
        <dl>
          <dt>client</dt>
          <dd>{e.client || "—"}</dd>
          <dt>dates</dt>
          <dd>{`${e.starts ?? "not set"} → ${e.ends ?? "not set"}`}</dd>
          <dt>expires</dt>
          <dd>{e.expires ?? "not set"}</dd>
          <dt>notes</dt>
          <dd className="pre">{e.notes || "—"}</dd>
        </dl>
      )}
    </Section>
  );
}

/** Out-of-band callbacks (#1): payloads and the hits on them. Hidden
 * unless a server is configured. A hit's raw request is shown as text. */
function OobPanel({ onOpenExchange }: { onOpenExchange(id: string): void }) {
  const [state, setState] = useState<OobState | string | null>(null);
  const [minted, setMinted] = useState<OobPayload | string | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let live = true;
    const load = () => void loadOob().then((s) => live && setState(s));
    load();
    // Callbacks can come late: look again now and then while it's open.
    const id = setInterval(load, 10_000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [nonce]);
  if (state === null || (typeof state !== "string" && !state.configured)) return null;
  return (
    <Section title="Out-of-band" label="Out-of-band">
      {typeof state === "string" ? (
        <ErrorLine msg={state} />
      ) : (
        <>
          <p className="note">{`Callbacks to *.${state.domain}: put a payload in a request; a DNS, HTTP or SMTP hit on it lands here, tied to the exchange that carried it.`}</p>
          <div className="inline">
            <button
              type="button"
              onClick={() =>
                void mintOob().then((p) => {
                  setMinted(p);
                  setNonce((n) => n + 1);
                })
              }
            >
              new payload
            </button>
            {typeof minted === "string" ? <span className="error">{minted}</span> : null}
          </div>
          {state.payloads.length ? (
            <ul className="entries oob-payloads" aria-label="Out-of-band payloads">
              {state.payloads.slice(0, 10).map((p) => (
                <li key={p.id}>
                  <span className="entry">{p.labUrl ?? p.host}</span>
                  <span className="note">{`${p.by} · ${p.at}`}</span>
                  <span className="row-actions">
                    <CopyButton text={p.labUrl ?? p.url} label={`Copy ${p.labUrl ? "the lab URL" : "the URL"} of ${p.id}`} />
                    {p.labUrl ? <CopyButton text={p.host} label={`Copy the host of ${p.id}`} caption="host" /> : null}
                  </span>
                </li>
              ))}
            </ul>
          ) : null}
          {state.hits.length ? (
            <ul className="entries oob-hits" aria-label="Out-of-band hits">
              {state.hits.map((h) => (
                <li key={h.id}>
                  <span className="entry">{`${h.id} · ${h.protocol} from ${h.remote}`}</span>
                  <span className="note">
                    {`${h.at} · ${h.payload}${h.task ? ` · ${h.task}` : ""}`}
                    {h.exchange ? (
                      <>
                        {" · "}
                        <button type="button" className="link" onClick={() => onOpenExchange(h.exchange!)}>
                          {h.exchange}
                        </button>
                      </>
                    ) : null}
                  </span>
                  <details>
                    <summary>raw request</summary>
                    <pre className="oob-raw">{h.rawRequest}</pre>
                  </details>
                </li>
              ))}
            </ul>
          ) : (
            <p className="none">{`No hits yet (${state.payloads.length} payload${state.payloads.length === 1 ? "" : "s"}).`}</p>
          )}
        </>
      )}
    </Section>
  );
}

/** Copy `text` to the clipboard: the async API, else the older
 * select-and-copy (some browsers and embedded views have only that). */
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the older way
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  let ok: boolean;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  return ok;
}

/** Copies `text`; says whether it worked. When the clipboard refuses, the
 * text is shown selected, to copy by hand. */
function CopyButton({ text, label, caption = "copy" }: { text: string; label: string; caption?: string }) {
  const [done, setDone] = useState<"ok" | "failed" | null>(null);
  return (
    <>
      <button type="button" aria-label={label} onClick={() => void copyText(text).then((ok) => setDone(ok ? "ok" : "failed"))}>
        {done === "ok" ? "copied" : caption}
      </button>
      {done === "failed" ? (
        <input
          className="copy-by-hand"
          aria-label={`${label}: the clipboard refused; it's selected here to copy`}
          readOnly
          value={text}
          size={Math.min(text.length, 48)}
          ref={(el) => el?.select()}
          onFocus={(ev) => ev.target.select()}
        />
      ) : null}
    </>
  );
}

/** Checks for sweep (#75): spall's vetted ones and this engagement's own. */
function ChecksPanel({ onChanged }: { onChanged(): void }) {
  const [list, setList] = useState<CheckInfo[] | string | null>(null);
  const [text, setText] = useState("");
  const [err, setErr] = useState<Result>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let live = true;
    void loadChecks().then((l) => live && setList(l));
    return () => {
      live = false;
    };
  }, [nonce]);
  const change = async (c: Parameters<typeof changeChecks>[0]) => {
    const r = await changeChecks(c);
    if (typeof r === "string") return setErr(r);
    setErr(null);
    if (c.op === "add") setText("");
    setNonce((n) => n + 1);
    onChanged();
  };
  return (
    <Section title="Checks" label="Checks">
      <p className="note">What `sweep &lt;base-url&gt; &lt;check&gt;` can run: paths relative to an in-scope base, GET or HEAD, judged against a soft-404 baseline.</p>
      {typeof list === "string" ? <ErrorLine msg={list} /> : null}
      {Array.isArray(list) ? (
        <ul className="entries" aria-label="Checks list">
          {list.map((c) => (
            <li key={c.name}>
              <span className="entry">{c.name}</span>
              <span className="note">{`${c.rule} · ${c.paths} ${c.method} paths · ${c.source === "engagement" ? "this engagement" : "built in"}`}</span>
              {c.source === "engagement" ? (
                <span className="row-actions">
                  <button type="button" aria-label={`Remove check ${c.name}`} onClick={() => void change({ op: "remove", name: c.name })}>
                    remove
                  </button>
                </span>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      <form className="check-import" onSubmit={(ev) => (ev.preventDefault(), void change({ op: "add", text }))}>
        <textarea
          aria-label="Import a check"
          rows={5}
          spellCheck={false}
          value={text}
          placeholder={"name my-check\nrule WSTG-CONF-04\nhit status 200\nhit differs\npath /old/index.php"}
          onChange={(ev) => setText(ev.target.value)}
        />
        <button type="submit" disabled={!text.trim()}>
          import
        </button>
      </form>
      <ErrorLine msg={err} />
    </Section>
  );
}

/** The global never-test list (#84), and this engagement's exceptions. */
function NeverPanel({ onChanged }: { onChanged(): void }) {
  const [list, setList] = useState<NeverList | string | null>(null);
  const [entry, setEntry] = useState("");
  const [err, setErr] = useState<Result>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let live = true;
    void loadNever().then((l) => live && setList(l));
    return () => {
      live = false;
    };
  }, [nonce]);
  const reload = () => {
    setNonce((n) => n + 1);
    onChanged();
  };
  const change = async (op: "add" | "remove", value: string) => {
    const r = await changeNever(op, value);
    if (typeof r === "string") return setErr(r);
    setErr(null);
    if (op === "add") setEntry("");
    reload();
  };
  const l = typeof list === "string" || list === null ? null : list;
  const missing = l ? SUGGESTED.filter((s) => !l.entries.includes(s)) : [];
  return (
    <Section title="Never tested" label="Never tested">
      <p className="note">Every engagement: refused after scope, even with every host allowed, and no approval reaches them. An engagement can be exempted from an entry.</p>
      {typeof list === "string" ? <ErrorLine msg={list} /> : null}
      {l && l.entries.length ? (
        <ul className="entries" aria-label="Never-test list">
          {l.entries.map((x) => (
            <li key={x}>
              <span className="entry">{x}</span>
              <label className="note">
                <input
                  type="checkbox"
                  checked={l.exceptions.includes(x)}
                  onChange={(ev) => {
                    const on = ev.target.checked;
                    // Shown at once; the server's answer follows on reload.
                    setList({ ...l, exceptions: on ? [...l.exceptions, x] : l.exceptions.filter((y) => y !== x) });
                    void setException(l, x, on).then((r) => {
                      setErr(r);
                      reload();
                    });
                  }}
                />
                {" allowed in this engagement"}
              </label>
              <span className="row-actions">
                <button type="button" aria-label={`Remove ${x} from the never-test list`} onClick={() => void change("remove", x)}>
                  remove
                </button>
              </span>
            </li>
          ))}
        </ul>
      ) : l ? (
        <p className="none">Empty.</p>
      ) : null}
      <form className="inline" onSubmit={(ev) => (ev.preventDefault(), void change("add", entry))}>
        <input aria-label="Never-test entry" placeholder="mycompany.example, *.okta.com, 203.0.113.0/24" value={entry} onChange={(ev) => setEntry(ev.target.value)} />
        <button type="submit" disabled={!entry.trim()}>
          add
        </button>
        {missing.length ? (
          <button type="button" onClick={() => void missing.reduce<Promise<unknown>>((p, s) => p.then(() => changeNever("add", s)), Promise.resolve()).then(reload)}>
            {`add ${missing.join(" and ")}`}
          </button>
        ) : null}
      </form>
      <ErrorLine msg={err} />
    </Section>
  );
}

function ScopePanel({ e, onChanged }: { e: EngagementInfo; onChanged(): void }) {
  const [entry, setEntry] = useState("");
  const [err, setErr] = useState<Result>(null);
  const [target, setTarget] = useState("");
  const [verdict, setVerdict] = useState<ScopeVerdict | string | null>(null);
  const [noteFor, setNoteFor] = useState<string | null>(null);
  const [note, setNote] = useState("");
  // `scope paste` / `scope import` in the console open these through
  // #engagement/paste and #engagement/import.
  const panelOf = () => (window.location.hash === "#engagement/paste" ? "paste" : window.location.hash === "#engagement/import" ? "import" : null);
  const [panel, setPanel] = useState<"paste" | "import" | null>(panelOf);
  useEffect(() => {
    const onHash = () => {
      const p = panelOf();
      if (p) setPanel(p);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const closePanel = () => {
    setPanel(null);
    if (panelOf()) window.history.replaceState(null, "", "#engagement");
  };
  const forgetSpec = async (i: number) => {
    const r = await send("/api/engagement", "PATCH", { api_specs: e.apiSpecs.filter((_, j) => j !== i) });
    setErr(r);
    if (!r) onChanged();
  };
  const inScope = e.scope.filter((x) => !x.startsWith("!"));
  const excluded = e.scope.filter((x) => x.startsWith("!")).map((x) => x.slice(1));
  const change = async (op: "add" | "exclude" | "remove", value: string, wide = false) => {
    const r = await send("/api/scope", "POST", wide ? { op, entry: value, wide: true } : { op, entry: value });
    setErr(r);
    if (!r) {
      if (op !== "remove") setEntry("");
      onChanged();
    }
  };
  const check = async (ev: FormEvent) => {
    ev.preventDefault();
    const body = await apiJson(`/api/scope/check?target=${encodeURIComponent(target)}`);
    if (typeof body === "string") return setVerdict(body);
    const o = body as { host?: unknown; state?: unknown; by?: unknown };
    setVerdict({
      host: typeof o.host === "string" ? o.host : target,
      state: o.state === "in" || o.state === "out" ? o.state : "unknown",
      by: typeof o.by === "string" ? o.by : null,
    });
  };
  const saveNote = async (key: string) => {
    const notes = { ...e.targetNotes };
    if (note.trim()) notes[key] = note.trim();
    else delete notes[key];
    const r = await send("/api/engagement", "PATCH", { target_notes: notes });
    setErr(r);
    if (!r) {
      setNoteFor(null);
      onChanged();
    }
  };
  const row = (shown: string, key: string) => (
    <li key={key}>
      <span className="entry">{shown}</span>
      {noteFor === key ? (
        <form className="inline" onSubmit={(ev) => (ev.preventDefault(), void saveNote(key))}>
          <input aria-label={`Note for ${shown}`} value={note} onChange={(ev) => setNote(ev.target.value)} autoFocus />
          <button type="submit">save</button>
        </form>
      ) : (
        <span className="note">{e.targetNotes[key] ?? ""}</span>
      )}
      <span className="row-actions">
        <button type="button" aria-label={`Note on ${shown}`} onClick={() => (setNoteFor(key), setNote(e.targetNotes[key] ?? ""))}>
          note
        </button>
        <button type="button" aria-label={`Remove ${shown}`} onClick={() => void change("remove", key)}>
          remove
        </button>
      </span>
    </li>
  );
  return (
    <Section
      title={`Scope · ${inScope.length} in, ${excluded.length} excluded`}
      label="Scope"
      actions={
        panel ? null : (
          <>
            <button type="button" onClick={() => setPanel("paste")}>
              paste a list
            </button>
            <button type="button" onClick={() => setPanel("import")}>
              import OpenAPI
            </button>
          </>
        )
      }
    >
      {panel === "paste" ? <ScopePaste onApplied={onChanged} onClose={closePanel} /> : null}
      {panel === "import" ? <OpenApiImport kept={e.apiSpecs} onApplied={onChanged} onClose={closePanel} /> : null}
      {inScope.length ? (
        <ul className="entries" aria-label="In scope">
          {inScope.map((x) => row(x, x))}
        </ul>
      ) : (
        <p className="empty">Nothing in scope: every replay is denied.</p>
      )}
      {excluded.length ? (
        <>
          <h4>Excluded (wins over the scope)</h4>
          <ul className="entries excluded" aria-label="Excluded">
            {excluded.map((x) => row(x, `!${x}`))}
          </ul>
        </>
      ) : null}
      <form className="inline" onSubmit={(ev) => (ev.preventDefault(), void change("add", entry))}>
        <input aria-label="Scope entry" placeholder="app.example.test, *.example.test, 10.20.0.0/16" value={entry} onChange={(ev) => setEntry(ev.target.value)} />
        <button type="submit" disabled={!entry.trim()}>
          add
        </button>
        <button type="button" disabled={!entry.trim()} onClick={() => void change("exclude", entry)}>
          exclude
        </button>
      </form>
      <ErrorLine msg={err} />
      {err && err.includes("too wide") && entry.trim() ? (
        <p className="wide-ok">
          <button type="button" onClick={() => void change("add", entry, true)}>
            {`add ${entry.trim()} anyway`}
          </button>
          <span className="note"> recorded in the audit log as too wide</span>
        </p>
      ) : null}
      {e.apiSpecs.length ? (
        <>
          <h4>API descriptions</h4>
          <ul className="entries" aria-label="API descriptions">
            {e.apiSpecs.map((a, i) => (
              <li key={i}>
                <span className="entry">{`${a.title}${a.version ? ` ${a.version}` : ""}`}</span>
                <span className="note" title={a.servers.join("\n")}>{`${a.format} · ${a.paths.length} paths · ${a.servers.length} servers`}</span>
                <span className="row-actions">
                  <button type="button" aria-label={`Forget ${a.title}`} onClick={() => void forgetSpec(i)}>
                    forget
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
      <form className="inline" onSubmit={(ev) => void check(ev)}>
        <input aria-label="Check a URL or host" placeholder="check a URL or host" value={target} onChange={(ev) => setTarget(ev.target.value)} />
        <button type="submit" disabled={!target.trim()}>
          check
        </button>
      </form>
      {verdict === null ? null : typeof verdict === "string" ? (
        <ErrorLine msg={verdict} />
      ) : (
        <p className={`verdict ${verdict.state}`} role="status">
          {verdict.state === "in"
            ? `${verdict.host}: in scope (by ${verdict.by ?? "?"})`
            : verdict.state === "out"
              ? `${verdict.host}: out of scope (excluded by ${verdict.by ?? "?"})`
              : `${verdict.host}: not in scope (no entry matches)`}
        </p>
      )}
    </Section>
  );
}

function RulesPanel({ e, onChanged }: { e: EngagementInfo; onChanged(): void }) {
  const r = e.rules;
  const [form, setForm] = useState<RulesForm | null>(null);
  const [err, setErr] = useState<Result>(null);
  const [win, setWin] = useState({ days: "mon-fri", span: "09:00-17:00" });
  const [bo, setBo] = useState({ from: "", to: "", note: "" });
  const apply = async (patch: Partial<Rules>) => {
    const res = await send("/api/engagement", "PATCH", { rules: patch });
    setErr(res);
    if (!res) onChanged();
    return res;
  };
  const save = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!form) return;
    const patch = rulesPatchOf(form, r);
    if (typeof patch === "string") return setErr(patch);
    if (Object.keys(patch).length && (await apply(patch))) return;
    setForm(null);
  };
  const addWindow = (ev: FormEvent) => {
    ev.preventDefault();
    const days = parseDays(win.days);
    const span = parseSpan(win.span);
    if (!days || !span) return setErr("testing window: days like mon-fri or sat,sun, and a time span like 09:00-17:00");
    void apply({ windows: [...r.windows, { days, ...span }] });
  };
  const addBlackout = (ev: FormEvent) => {
    ev.preventDefault();
    if (!bo.from || !bo.to) return setErr("no-test window: a start and an end");
    void apply({ blackouts: [...r.blackouts, { from: bo.from, to: bo.to, note: bo.note }] }).then((res) => {
      if (!res) setBo({ from: "", to: "", note: "" });
    });
  };
  return (
    <Section
      title="Rules of engagement"
      actions={
        form ? null : (
          <button type="button" onClick={() => setForm(rulesFormOf(r))}>
            edit
          </button>
        )
      }
    >
      {form ? (
        <form className="eng-form" onSubmit={(ev) => void save(ev)}>
          <label>
            <span>Methods allowed</span>
            <input value={form.methods} onChange={(ev) => setForm({ ...form, methods: ev.target.value })} />
          </label>
          <label>
            <span>Sent without confirmation</span>
            <input value={form.safe} onChange={(ev) => setForm({ ...form, safe: ev.target.value })} />
          </label>
          <label>
            <span>Rate ceiling (per second)</span>
            <input value={form.rate} onChange={(ev) => setForm({ ...form, rate: ev.target.value })} />
          </label>
          <label>
            <span>Sweep size (requests, 0: no sweeps)</span>
            <input inputMode="numeric" value={form.sweepMax} onChange={(ev) => setForm({ ...form, sweepMax: ev.target.value })} />
          </label>
          <label>
            <span>Time zone</span>
            <input value={form.tz} onChange={(ev) => setForm({ ...form, tz: ev.target.value })} />
          </label>
          <label className="check">
            <input type="checkbox" checked={form.agents} onChange={(ev) => setForm({ ...form, agents: ev.target.checked })} />
            <span>Agents may act</span>
          </label>
          <label className="check">
            <input type="checkbox" checked={form.destructive} onChange={(ev) => setForm({ ...form, destructive: ev.target.checked })} />
            <span>Destructive actions allowed</span>
          </label>
          <label className="check">
            <input type="checkbox" checked={form.refuseLooking} onChange={(ev) => setForm({ ...form, refuseLooking: ev.target.checked })} />
            <span>Refuse requests that look destructive (by path or payload) instead of asking</span>
          </label>
          <div className="actions">
            <button type="submit">save</button>
            <button type="button" onClick={() => (setForm(null), setErr(null))}>
              cancel
            </button>
          </div>
        </form>
      ) : (
        <dl>
          <dt>methods</dt>
          <dd>{r.methods.length ? r.methods.join(", ") : "any"}</dd>
          <dt>no confirmation</dt>
          <dd>{r.safe_methods.length ? r.safe_methods.join(", ") : "none: every method is confirmed"}</dd>
          <dt>rate</dt>
          <dd>{r.rate_limit === null ? "no ceiling" : `${r.rate_limit} per second`}</dd>
          <dt>time zone</dt>
          <dd>{r.time_zone}</dd>
          <dt>agents</dt>
          <dd>{r.agents_allowed ? "allowed" : "not allowed"}</dd>
          <dt>destructive</dt>
          <dd>{r.destructive_allowed ? "allowed" : "not allowed"}</dd>
          <dt>looks destructive</dt>
          <dd>{r.refuse_destructive_looking ? "refused" : "asked for first"}</dd>
          <dt>sweeps</dt>
          <dd>{r.sweep_max ? `up to ${r.sweep_max} requests` : "not allowed"}</dd>
        </dl>
      )}
      <h4>Testing windows</h4>
      {r.windows.length ? (
        <ul className="entries" aria-label="Testing windows">
          {r.windows.map((w, i) => (
            <li key={i}>
              <span className="entry">{`${daysText(w.days)} ${w.start}-${w.end}`}</span>
              <span className="note" />
              <span className="row-actions">
                <button type="button" aria-label={`Remove window ${i + 1}`} onClick={() => void apply({ windows: r.windows.filter((_, j) => j !== i) })}>
                  remove
                </button>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="empty">Any time.</p>
      )}
      <form className="inline" onSubmit={addWindow}>
        <input aria-label="Window days" value={win.days} onChange={(ev) => setWin({ ...win, days: ev.target.value })} size={10} />
        <input aria-label="Window times" value={win.span} onChange={(ev) => setWin({ ...win, span: ev.target.value })} size={12} />
        <button type="submit">add window</button>
      </form>
      <h4>No-test windows</h4>
      {r.blackouts.length ? (
        <ul className="entries" aria-label="No-test windows">
          {r.blackouts.map((b, i) => (
            <li key={i}>
              <span className="entry">{`${b.from.replace("T", " ")} → ${b.to.replace("T", " ")}`}</span>
              <span className="note">{b.note}</span>
              <span className="row-actions">
                <button type="button" aria-label={`Remove no-test window ${i + 1}`} onClick={() => void apply({ blackouts: r.blackouts.filter((_, j) => j !== i) })}>
                  remove
                </button>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="empty">None.</p>
      )}
      <form className="inline" onSubmit={addBlackout}>
        <input type="datetime-local" aria-label="No-test from" value={bo.from} onChange={(ev) => setBo({ ...bo, from: ev.target.value })} />
        <input type="datetime-local" aria-label="No-test to" value={bo.to} onChange={(ev) => setBo({ ...bo, to: ev.target.value })} />
        <input aria-label="No-test note" placeholder="note" value={bo.note} onChange={(ev) => setBo({ ...bo, note: ev.target.value })} />
        <button type="submit">add no-test window</button>
      </form>
      <ErrorLine msg={err} />
    </Section>
  );
}

const STATE_WORD: Record<HostState, string> = { in: "in scope", out: "excluded", unknown: "undecided" };

/** Hosts seen in traffic (#39): decide each unknown one in a click. */
function HostsPanel({ listing, onChanged }: { listing: HostListing | null; onChanged(): void }) {
  const [all, setAll] = useState(false);
  const [err, setErr] = useState<Result>(null);
  if (!listing) return null;
  const decide = async (op: "add" | "exclude", entry: string) => {
    const r = await send("/api/scope", "POST", { op, entry });
    setErr(r);
    if (!r) onChanged();
  };
  const shown = all ? listing.hosts : listing.hosts.filter((h) => h.state === "unknown");
  const c = listing.counts;
  return (
    <Section
      title={`Hosts from traffic · ${c.unknown} undecided`}
      label="Hosts from traffic"
      actions={
        <label className="check">
          <input type="checkbox" checked={all} onChange={(ev) => setAll(ev.target.checked)} /> all
        </label>
      }
    >
      {listing.hosts.some((h) => h.by === "*") ? (
        <p className="note wrap">Scope all is on: every host counts as in scope, so none are undecided.</p>
      ) : null}
      <p className="note wrap">{`${c.in} in scope · ${c.out} excluded · ${c.unknown} undecided, from recent and live traffic${listing.dropped ? ` (${listing.dropped} more not tracked)` : ""}`}</p>
      {shown.length ? (
        <ul className="entries hosts" aria-label={all ? "Hosts" : "Undecided hosts"}>
          {shown.map((h) => {
            const parent = parentWildcard(h.host);
            return (
              <li key={h.host} className={`h-${h.state}`}>
                <span className="entry">
                  <span className={`sc sc-${h.state}`} aria-hidden>
                    ●
                  </span>
                  {h.host}
                </span>
                <span className="note" title={`first seen ${h.firstSeen}, last seen ${h.lastSeen}`}>
                  {`${h.count}× · last ${timeOf(h.lastSeen)}${h.state === "unknown" ? "" : ` · ${STATE_WORD[h.state]}${h.by ? ` by ${h.by}` : ""}`}`}
                </span>
                {h.state === "unknown" ? (
                  <span className="row-actions">
                    <button type="button" aria-label={`Add ${h.host} to scope`} onClick={() => void decide("add", h.host)}>
                      add
                    </button>
                    <button type="button" aria-label={`Exclude ${h.host}`} onClick={() => void decide("exclude", h.host)}>
                      exclude
                    </button>
                    {parent ? (
                      <button type="button" aria-label={`Exclude ${parent}`} onClick={() => void decide("exclude", parent)}>
                        {`exclude ${parent}`}
                      </button>
                    ) : null}
                  </span>
                ) : (
                  <span />
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="empty">{all ? "No traffic yet." : "Nothing undecided: every host seen is in scope or excluded."}</p>
      )}
      <ErrorLine msg={err} />
    </Section>
  );
}

/** Browser setup (#32); `ca` in the console and the empty Traffic view link here. */
function SetupSection({ proxy, latestHttps }: { proxy: string | null; latestHttps: TrafficEntry | null }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const go = () => {
      if (window.location.hash === "#engagement/ca") ref.current?.scrollIntoView({ block: "start" });
    };
    go();
    window.addEventListener("hashchange", go);
    return () => window.removeEventListener("hashchange", go);
  }, []);
  return (
    <div ref={ref} id="browser-setup">
      <Section title="Set up a browser">
        <BrowserSetup proxy={proxy} latestHttps={latestHttps} />
      </Section>
    </div>
  );
}

/** Shown at most, oldest first; the rest are in Findings. */
const WAITING_SHOWN = 5;

/** Coverage (#94): in-scope endpoints and how many are untested per WSTG category; the Surface has the rest. */
function Coverage({ nonce }: { nonce: number }) {
  const [s, setS] = useState<SurfaceListing | string | null>(null);
  useEffect(() => {
    let live = true;
    void loadSurface(false).then((l) => live && setS(l));
    return () => {
      live = false;
    };
  }, [nonce]);
  if (s === null) return null;
  if (typeof s === "string") return <p className="error">{`coverage: ${s}`}</p>;
  const t = totals(s.rows, s.categories);
  const tested = s.rows.filter((r) => Object.values(r.coverage).some((c) => c.state === "tested")).length;
  return (
    <div className="waiting coverage" role="group" aria-label="Coverage">
      <h4>
        <a href="#surface">{`Coverage · ${s.rows.length} endpoints in scope, ${tested} tested`}</a>
      </h4>
      {s.rows.length === 0 ? (
        <p className="note">No in-scope endpoints yet: browse the app through the proxy, or import its OpenAPI description.</p>
      ) : (
        <p className="note cov-sum">
          {s.categories.map((c) => `${c.code} ${t[c.code]?.untested ?? 0}`).join(" · ")}
          <span>{" untested"}</span>
        </p>
      )}
    </div>
  );
}

function Waiting({ toTriage, waitingApproval, onChanged }: { toTriage: number | null; waitingApproval: number | null; onChanged(): void }) {
  const [list, setList] = useState<Candidate[] | string | null>(null);
  useEffect(() => {
    let live = true;
    void listCandidates().then((l) => live && setList(l));
    return () => {
      live = false;
    };
  }, [toTriage]);
  const waiting = Array.isArray(list) ? list.filter((c) => stateOf(c) === "to triage").reverse() : [];
  return (
    <Section title="Waiting on you">
      <div className="waiting" role="group" aria-label="Candidates to triage">
        <h4>
          <a href="#findings">{`Candidates to triage · ${waiting.length}`}</a>
        </h4>
        {typeof list === "string" ? <p className="error">{list}</p> : null}
        {Array.isArray(list) && waiting.length === 0 ? <p className="note">Nothing waiting: the queue is clear.</p> : null}
        {waiting.length ? (
          <ol className="entries">
            {waiting.slice(0, WAITING_SHOWN).map((c) => (
              <li key={c.id}>
                <a href={`#findings/${encodeURIComponent(c.id)}`}>{`${c.id} · ${c.ruleId}`}</a>
                <span className="note" title={where(c)}>{`${where(c)} · ${timeOf(c.created)}`}</span>
              </li>
            ))}
          </ol>
        ) : null}
        {waiting.length > WAITING_SHOWN ? <a href="#findings">{`and ${waiting.length - WAITING_SHOWN} more in Findings`}</a> : null}
      </div>
      <Approvals count={waitingApproval} onChanged={onChanged} />
      <Activity nonce={(toTriage ?? 0) + (waitingApproval ?? 0)} />
      <Coverage nonce={toTriage ?? 0} />
    </Section>
  );
}


/** The newest audit records, yours and the agents' interleaved (#62, first cut). */
function Activity({ nonce }: { nonce: number }) {
  const [lines, setLines] = useState<ActivityLine[] | string | null>(null);
  useEffect(() => {
    let live = true;
    void apiJson("/api/audit?last=15").then((b) => {
      if (!live) return;
      if (typeof b === "string") setLines(b);
      else setLines(Array.isArray(b) ? (b as Record<string, unknown>[]).map(activityOf).reverse() : []);
    });
    return () => {
      live = false;
    };
  }, [nonce]);
  return (
    <div className="activity" role="group" aria-label="Activity">
      <h4>Activity</h4>
      {typeof lines === "string" ? <p className="error">{lines}</p> : null}
      {Array.isArray(lines) && lines.length === 0 ? <p className="note">Nothing yet.</p> : null}
      {Array.isArray(lines) && lines.length ? (
        <ol className="entries">
          {lines.map((l, i) => (
            <li key={i} className={l.who.agent ? "agent" : undefined}>
              <span className="note">{l.time}</span>
              <By who={l.who} verb="" />
              <span>{l.what}</span>
            </li>
          ))}
        </ol>
      ) : null}
    </div>
  );
}
