// The Findings view (#44, docs/ux.md §5): a triage inbox you can clear from
// the keyboard. A queue of candidates on the left; the selected one on the
// right, in the inspector's place. c confirms; r rejects, a accepts the
// risk and d marks a duplicate, each with a justification; o reopens.
// Promotion and export are M5. Every string shown came from captured
// traffic or a tester: text only.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { change, listCandidates, LEVELS, stateOf, where, type Candidate, type Level, type Op } from "../lib/candidates.ts";
import { candidateFromHash, duplicateTargets, nextAfter, queue, tabCounts, TABS, type Tab } from "../lib/findings.ts";
import { loadCatalog, type Rule } from "../lib/rules.ts";
import { timeOf } from "../lib/format.ts";
import { Evidence } from "./Evidence.tsx";

type Asking = { kind: "reject" | "accept" | "duplicate" | "edit" } | null;

const ASK_LABEL = { reject: "Reject (not a real issue)", accept: "Accept the risk", duplicate: "Duplicate of another candidate", edit: "Edit" } as const;

export function FindingsView({
  available,
  onChanged,
  onOpenExchange,
}: {
  available: boolean;
  /** A candidate changed (status bar counts). */
  onChanged(): void;
  /** Open an exchange in Traffic's inspector. */
  onOpenExchange(id: string): void;
}) {
  const [list, setList] = useState<Candidate[] | string | null>(null);
  const [tab, setTab] = useState<Tab>(() => (candidateFromHash(window.location.hash) ? "all" : "to triage"));
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(() => candidateFromHash(window.location.hash));
  const [asking, setAsking] = useState<Asking>(null);
  const [msg, setMsg] = useState<{ text: string; bad: boolean } | null>(null);
  const [rules, setRules] = useState<Rule[]>([]);
  const filterRef = useRef<HTMLInputElement>(null);

  const reload = useCallback(async () => {
    const l = await listCandidates();
    setList(l);
    return l;
  }, []);

  useEffect(() => {
    if (!available) return;
    let live = true;
    const poll = () => void listCandidates().then((l) => live && setList(l));
    poll();
    void loadCatalog().then((c) => live && typeof c !== "string" && setRules(c.rules));
    const id = setInterval(poll, 10_000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [available]);

  // A link to #findings/<id> (from the inspector or the console) selects it
  // and shows it whatever tab it's on.
  useEffect(() => {
    const onHash = () => {
      const id = candidateFromHash(window.location.hash);
      if (id) {
        setSelected(id);
        setTab("all");
      }
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const all = useMemo(() => (Array.isArray(list) ? list : []), [list]);
  const shown = useMemo(() => queue(all, tab, query), [all, tab, query]);
  const counts = useMemo(() => tabCounts(all), [all]);
  // The selection, or the top of the queue while there's anything to select.
  const sel = selected && all.some((c) => c.id === selected) ? selected : (shown[0]?.id ?? null);
  const current = all.find((c) => c.id === sel) ?? null;

  const act = useCallback(
    async (op: Op) => {
      if (!current) return;
      const before = shown;
      const res = await change(current.id, op);
      if (typeof res === "string") {
        setMsg({ text: res, bad: true });
        return;
      }
      setAsking(null);
      setMsg({ text: `${res.id}: ${op.op === "evidence" ? "evidence added" : stateOf(res)}`, bad: false });
      onChanged();
      const fresh = await reload();
      if (Array.isArray(fresh)) setSelected(nextAfter(before, queue(fresh, tab, query), res.id));
    },
    [current, shown, onChanged, reload, tab, query],
  );

  const move = useCallback(
    (d: 1 | -1) => {
      if (!shown.length) return;
      const at = shown.findIndex((c) => c.id === sel);
      const next = shown[Math.min(shown.length - 1, Math.max(0, at < 0 ? 0 : at + d))]!;
      setSelected(next.id);
      setAsking(null);
    },
    [shown, sel],
  );

  // The inbox's keys. Ignored while typing, and with modifiers.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if ((e.target as HTMLElement | null)?.closest("input, textarea, select, [role='dialog'], .console")) return;
      const k = e.key;
      if (k === "j" || k === "ArrowDown") move(1);
      else if (k === "k" || k === "ArrowUp") move(-1);
      else if (k === "/") {
        e.preventDefault();
        filterRef.current?.focus();
      } else if (!current) return;
      else if (k === "c" && !current.suppressed && current.status !== "confirmed") void act({ op: "confirm" });
      else if (k === "r" || k === "a" || k === "d") {
        e.preventDefault();
        setAsking({ kind: k === "r" ? "reject" : k === "a" ? "accept" : "duplicate" });
      } else if (k === "o" && current.suppressed) void act({ op: "reopen" });
      else if (k === "Escape" && asking) setAsking(null);
      else return;
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [move, act, current, asking]);

  if (!available) {
    return (
      <section className="findings" aria-label="Findings">
        <p className="empty">Candidates need taphttp's lever API (start spall with --taphttp-api).</p>
      </section>
    );
  }

  return (
    <section className="findings" aria-label="Findings">
      <div className="queue">
        <div className="toolbar">
          <input
            ref={filterRef}
            type="search"
            className="filter"
            aria-label="Filter candidates"
            placeholder="filter   rule:INPV  host:shop  state:rejected  level:error  text"
            value={query}
            onChange={(ev) => setQuery(ev.target.value)}
            onKeyDown={(ev) => ev.key === "Escape" && ev.currentTarget.blur()}
          />
        </div>
        <div className="qtabs" role="tablist" aria-label="Queue">
          {TABS.map((t) => (
            <button key={t} type="button" role="tab" aria-selected={tab === t} className={tab === t ? "on" : undefined} onClick={() => setTab(t)}>
              {t}
              <span className="count">{counts[t]}</span>
            </button>
          ))}
        </div>
        {typeof list === "string" ? (
          <p className="error" role="alert">
            {list}
          </p>
        ) : null}
        {list === null ? <p className="empty">loading…</p> : null}
        {Array.isArray(list) && shown.length === 0 ? (
          <p className="empty">
            {all.length === 0
              ? "No candidates yet. Flag an exchange from Traffic's inspector (f), or flag <id> --rule … in the console."
              : tab === "to triage" && !query
                ? "Nothing waiting for you: the queue is clear."
                : "Nothing here matches."}
          </p>
        ) : null}
        <ol className="qlist" aria-label="Candidates">
          {shown.map((c) => (
            <li key={c.id} className={c.id === sel ? "sel" : undefined} aria-current={c.id === sel ? "true" : undefined}>
              <button type="button" onClick={() => (setSelected(c.id), setAsking(null))}>
                <span className={`lvl lvl-${c.level ?? "none"}`}>{c.level ?? "—"}</span>
                <span className="rule">{c.ruleId}</span>
                <span className="where">{where(c)}</span>
                <span className={`state st-${stateOf(c).replace(/ /g, "-")}`}>{stateOf(c)}</span>
              </button>
            </li>
          ))}
        </ol>
        <p className="note keys">j/k move · c confirm · r reject · a accept risk · d duplicate · o reopen · / filter</p>
      </div>

      {current ? (
        <aside className="inspector cand" aria-label="Candidate">
          <header>
            <h2>{`${current.id} · ${current.ruleId}`}</h2>
            <div className="meta">
              <span className={`state st-${stateOf(current).replace(/ /g, "-")}`}>{stateOf(current)}</span>
              <span>{current.level ?? "no severity"}</span>
              {current.rank !== null ? <span>{`confidence ${current.rank}`}</span> : null}
              {current.cwes.map((c) => (
                <span key={c}>{`CWE-${c}`}</span>
              ))}
              <span>{`flagged by ${current.flaggedBy || "?"} ${timeOf(current.created)}`}</span>
            </div>
          </header>
          <div className="actions">
            <button type="button" disabled={!!current.suppressed || current.status === "confirmed"} onClick={() => void act({ op: "confirm" })}>
              Confirm <kbd>c</kbd>
            </button>
            <button type="button" onClick={() => setAsking({ kind: "reject" })}>
              Reject <kbd>r</kbd>
            </button>
            <button type="button" onClick={() => setAsking({ kind: "accept" })}>
              Accept risk <kbd>a</kbd>
            </button>
            <button type="button" onClick={() => setAsking({ kind: "duplicate" })}>
              Duplicate <kbd>d</kbd>
            </button>
            {current.suppressed ? (
              <button type="button" onClick={() => void act({ op: "reopen" })}>
                Reopen <kbd>o</kbd>
              </button>
            ) : null}
            <button type="button" onClick={() => setAsking({ kind: "edit" })}>
              Edit
            </button>
            {msg ? (
              <p className={msg.bad ? "replay-msg" : "ok-msg"} role="status">
                {msg.text}
              </p>
            ) : null}
          </div>
          <section className="pane">
            {asking ? (
              <Ask
                key={`${current.id}-${asking.kind}`}
                kind={asking.kind}
                current={current}
                targets={duplicateTargets(all, current.id)}
                onCancel={() => setAsking(null)}
                onSubmit={(op) => void act(op)}
              />
            ) : null}
            <h3>Finding</h3>
            <p className="msg">{current.message}</p>
            <dl className="kv">
              <dt>rule</dt>
              <dd>
                {(() => {
                  const r = rules.find((x) => x.id === current.ruleId);
                  return r ? `${r.id} ${r.title}` : current.ruleId;
                })()}
              </dd>
              <dt>where</dt>
              <dd>{where(current) || "—"}</dd>
              {current.guid ? (
                <>
                  <dt>correlation</dt>
                  <dd>{current.guid}</dd>
                </>
              ) : null}
              {current.suppressed ? (
                <>
                  <dt>decision</dt>
                  <dd className="why">{`${stateOf(current)}: ${current.suppressed.justification}`}</dd>
                </>
              ) : null}
              <dt>updated</dt>
              <dd>{timeOf(current.updated)}</dd>
            </dl>
            <Evidence key={current.id} candidate={current} onOpenExchange={onOpenExchange} onAdd={(ids) => void act({ op: "evidence", exchanges: ids })} />
            <p className="note wrap">Export as flux, redacted, comes in M5. Promotion is always a human's call.</p>
          </section>
        </aside>
      ) : null}
    </section>
  );
}

/** The justification (and for a duplicate, which one) a decision needs. */
function Ask({
  kind,
  current,
  targets,
  onCancel,
  onSubmit,
}: {
  kind: "reject" | "accept" | "duplicate" | "edit";
  current: Candidate;
  targets: Candidate[];
  onCancel(): void;
  onSubmit(op: Op): void;
}) {
  const [why, setWhy] = useState("");
  const [of, setOf] = useState(targets[0]?.id ?? "");
  const [level, setLevel] = useState<Level>(current.level ?? "warning");
  const [rank, setRank] = useState(current.rank === null ? "" : String(current.rank));
  const [message, setMessage] = useState(current.message);
  const [err, setErr] = useState<string | null>(null);

  const submit = () => {
    if (kind === "edit") {
      const r = rank.trim() === "" ? undefined : Number(rank);
      if (r !== undefined && (!Number.isFinite(r) || r < 0 || r > 100)) return setErr("confidence: 0-100");
      if (!message.trim()) return setErr("message: required");
      return onSubmit({ op: "edit", level, rank: r, message: message.trim() });
    }
    if (!why.trim()) return setErr("a justification is required");
    if (kind === "duplicate") {
      if (!of) return setErr("no other candidate to be a duplicate of");
      return onSubmit({ op: "duplicate", of, justification: why.trim() });
    }
    onSubmit({ op: kind, justification: why.trim() });
  };

  return (
    <form
      className="ask"
      aria-label={ASK_LABEL[kind]}
      onSubmit={(ev) => {
        ev.preventDefault();
        submit();
      }}
      onKeyDown={(ev) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          onCancel();
        } else if (ev.key === "Enter" && !ev.shiftKey && (ev.target as HTMLElement).tagName === "TEXTAREA") {
          ev.preventDefault();
          submit();
        }
      }}
    >
      <h3>{ASK_LABEL[kind]}</h3>
      {kind === "duplicate" ? (
        <label>
          Of
          <select aria-label="Duplicate of" value={of} onChange={(ev) => setOf(ev.target.value)}>
            {targets.map((t) => (
              <option key={t.id} value={t.id}>
                {`${t.id} · ${t.ruleId} · ${stateOf(t)} · ${where(t)}`}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {kind === "edit" ? (
        <>
          <label>
            Severity
            <select aria-label="Severity" value={level} onChange={(ev) => setLevel(ev.target.value as Level)}>
              {LEVELS.map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </select>
          </label>
          <label>
            Confidence (0-100)
            <input aria-label="Confidence" value={rank} onChange={(ev) => setRank(ev.target.value)} />
          </label>
          <label>
            Message
            <textarea aria-label="Message" rows={3} value={message} onChange={(ev) => setMessage(ev.target.value)} />
          </label>
        </>
      ) : (
        <label>
          Justification
          <textarea
            autoFocus
            aria-label="Justification"
            rows={2}
            value={why}
            placeholder={kind === "reject" ? "why it isn't real (kept with the decision)" : kind === "accept" ? "who accepted it, and why" : "why it's the same issue"}
            onChange={(ev) => setWhy(ev.target.value)}
          />
        </label>
      )}
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
      <div className="row-actions">
        <button type="submit">{kind === "edit" ? "save" : kind === "duplicate" ? "mark duplicate" : kind} </button>
        <button type="button" onClick={onCancel}>
          cancel <kbd>Esc</kbd>
        </button>
      </div>
    </form>
  );
}
