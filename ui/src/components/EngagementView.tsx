// The Engagement view (#37, docs/ux.md §1): what the test is, its scope and
// its rules, and (later) what's waiting on you. Every change goes through
// the same audited API as the console. Engagement details are typed by the
// tester, but everything here is still rendered as text.

import { useState, type FormEvent, type ReactNode } from "react";
import type { EngagementInfo, ScopeVerdict } from "../commands/registry.ts";
import { apiJson, expiryText } from "../lib/engagement.ts";
import { daysText, parseDays, parseSpan, rulesFormOf, rulesPatchOf, type Rules, type RulesForm } from "../lib/roe.ts";
import { useEngagement } from "../useEngagement.ts";

type Result = string | null;

/** Run a change; a string result is an error message. */
async function send(path: string, method: string, body: unknown): Promise<Result> {
  const res = await apiJson(path, { method, body });
  return typeof res === "string" ? res : null;
}

export function EngagementView({ available, onChanged }: { available: boolean; onChanged(): void }) {
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
        <RulesPanel e={e} onChanged={done} />
        <Waiting />
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

function ScopePanel({ e, onChanged }: { e: EngagementInfo; onChanged(): void }) {
  const [entry, setEntry] = useState("");
  const [err, setErr] = useState<Result>(null);
  const [target, setTarget] = useState("");
  const [verdict, setVerdict] = useState<ScopeVerdict | string | null>(null);
  const [noteFor, setNoteFor] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const inScope = e.scope.filter((x) => !x.startsWith("!"));
  const excluded = e.scope.filter((x) => x.startsWith("!")).map((x) => x.slice(1));
  const change = async (op: "add" | "exclude" | "remove", value: string) => {
    const r = await send("/api/scope", "POST", { op, entry: value });
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
    <Section title={`Scope · ${inScope.length} in, ${excluded.length} excluded`} label="Scope">
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
        <input aria-label="Scope entry" placeholder="app.example.test, *.example.test, 10.0.0.0/8" value={entry} onChange={(ev) => setEntry(ev.target.value)} />
        <button type="submit" disabled={!entry.trim()}>
          add
        </button>
        <button type="button" disabled={!entry.trim()} onClick={() => void change("exclude", entry)}>
          exclude
        </button>
      </form>
      <ErrorLine msg={err} />
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

/** Panels that grow later: dimmed and inert until they ship (docs/ux.md). */
function Waiting() {
  const todo: [string, string][] = [
    ["Candidates to triage", "M4"],
    ["Tasks waiting on approval", "M6"],
    ["Coverage: endpoints seen vs. tested", "Surface"],
    ["Set up a browser (CA certificate)", "#32"],
  ];
  return (
    <Section title="Waiting on you">
      <ul className="todo-panels">
        {todo.map(([what, when]) => (
          <li key={what} className="todo" aria-disabled="true" title={`${what} · not built yet (${when})`}>
            <span>{what}</span>
            <span className="note">{`not built yet · ${when}`}</span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

