// Flag as candidate (#43): in the inspector, never on a row. The tester
// picks a rule from the catalog (#41) and says which parameter; spall
// builds the flux finding server-side. A match on an existing candidate's
// fingerprint offers to add this exchange as evidence, or, if it was
// rejected before, says so and offers to reopen it (#46). Text only.

import { useEffect, useMemo, useState } from "react";
import { change, flag, LEVELS, paramNames, stateOf, type Candidate, type Level } from "../lib/candidates.ts";
import { loadCatalog, OTHER, ruleLine, searchRules, type Rule } from "../lib/rules.ts";
import { shortId } from "../lib/format.ts";
import type { TrafficEntry } from "../types.ts";

type Outcome = { kind: "created" | "evidence" | "reopened"; candidate: Candidate } | { kind: "exists"; candidate: Candidate; similar: string | null } | null;

export function FlagForm({
  entry,
  replays,
  onCancel,
  onChanged,
}: {
  entry: TrafficEntry;
  replays: TrafficEntry[];
  onCancel(): void;
  /** A candidate was created or changed. */
  onChanged(c: Candidate): void;
}) {
  const [rules, setRules] = useState<Rule[] | string | null>(null);
  const [search, setSearch] = useState("");
  const [rule, setRule] = useState("");
  const [parameter, setParameter] = useState("");
  const [level, setLevel] = useState<Level>("warning");
  const [rank, setRank] = useState("");
  const [message, setMessage] = useState("");
  const [cwe, setCwe] = useState("");
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome>(null);

  useEffect(() => {
    let live = true;
    void loadCatalog().then((c) => live && setRules(typeof c === "string" ? c : c.rules));
    return () => {
      live = false;
    };
  }, []);

  const shown = useMemo(() => (Array.isArray(rules) ? searchRules(rules, search) : []), [rules, search]);
  const params = useMemo(() => paramNames(entry.url), [entry.url]);
  const chosen = Array.isArray(rules) ? rules.find((r) => r.id === rule) : undefined;

  const submit = async (force = false) => {
    if (!rule) return setErr("pick a rule");
    const r = rank.trim() ? Number(rank) : undefined;
    if (r !== undefined && (!Number.isFinite(r) || r < 0 || r > 100)) return setErr("confidence: 0-100");
    const c = cwe.trim() ? Number(cwe.trim().replace(/^cwe-/i, "")) : undefined;
    if (c !== undefined && (!Number.isInteger(c) || c <= 0)) return setErr("CWE: a number");
    setBusy(true);
    setErr(null);
    const res = await flag({
      exchange: entry.id,
      rule,
      parameter,
      level,
      rank: r,
      message,
      replays: [...picked],
      cwe: rule === OTHER ? c : undefined,
      force,
    });
    setBusy(false);
    if (res.kind === "error") return setErr(res.message);
    setOutcome(res.kind === "created" ? { kind: "created", candidate: res.candidate } : { kind: "exists", candidate: res.candidate, similar: res.similar ? res.message : null });
    if (res.kind === "created") onChanged(res.candidate);
  };

  const follow = async (c: Candidate, how: "evidence" | "reopen") => {
    setBusy(true);
    const res = await change(c.id, how === "evidence" ? { op: "evidence", exchanges: [entry.id, ...picked] } : { op: "reopen" });
    setBusy(false);
    if (typeof res === "string") return setErr(res);
    setOutcome({ kind: how === "evidence" ? "evidence" : "reopened", candidate: res });
    onChanged(res);
  };

  if (outcome) {
    const c = outcome.candidate;
    const state = stateOf(c);
    return (
      <div className="flag" role="group" aria-label="Flag as candidate">
        {outcome.kind === "exists" ? (
          outcome.similar ? (
            <>
              <p className="badge-prev" role="status">{`Possibly a duplicate of ${c.id} (${c.ruleId}, ${state})`}</p>
              <p className="note wrap">{outcome.similar}</p>
            </>
          ) : c.suppressed ? (
            <>
              <p className="badge-prev" role="status">{`Previously ${state}: ${c.id} (${c.ruleId})`}</p>
              <p className="why">{c.suppressed.justification}</p>
              <p className="note wrap">The same endpoint, parameter and rule were decided before, so this isn't raised again. Reopen it if the decision no longer holds.</p>
            </>
          ) : (
            <p role="status">{`Already flagged as ${c.id} (${c.ruleId}, ${state}). Add this exchange to its evidence instead?`}</p>
          )
        ) : (
          <p className="ok" role="status">
            {outcome.kind === "created"
              ? `✓ Flagged as ${c.id}: ${c.ruleId}, waiting for triage.`
              : outcome.kind === "evidence"
                ? `✓ Added to ${c.id}'s evidence (${c.exchanges.length} exchanges).`
                : `✓ Reopened ${c.id}: back in the queue.`}
          </p>
        )}
        {err ? (
          <p className="error" role="alert">
            {err}
          </p>
        ) : null}
        <div className="row-actions">
          {outcome.kind === "exists" && (!c.suppressed || outcome.similar) ? (
            <button type="button" disabled={busy || c.exchanges.includes(entry.id)} onClick={() => void follow(c, "evidence")}>
              {c.exchanges.includes(entry.id) ? "this exchange is already evidence" : "add as evidence"}
            </button>
          ) : null}
          {outcome.kind === "exists" && outcome.similar ? (
            <button type="button" disabled={busy} onClick={() => void submit(true)}>
              flag anyway
            </button>
          ) : null}
          {outcome.kind === "exists" && c.suppressed && !outcome.similar ? (
            <button type="button" disabled={busy} onClick={() => void follow(c, "reopen")}>
              reopen it
            </button>
          ) : null}
          <a className="button" href={`#findings/${encodeURIComponent(c.id)}`}>
            open in Findings
          </a>
          <button type="button" onClick={onCancel}>
            close
          </button>
        </div>
      </div>
    );
  }

  return (
    <form
      className="flag"
      aria-label="Flag as candidate"
      onSubmit={(ev) => {
        ev.preventDefault();
        void submit();
      }}
      onKeyDown={(ev) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          onCancel();
        } else if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) {
          ev.preventDefault();
          void submit();
        }
      }}
    >
      <h3>
        Flag as candidate <span className="note">{`· ${entry.method} ${shortId(entry.id)}`}</span>
      </h3>
      {rules === null ? <p className="none">loading rules…</p> : null}
      {typeof rules === "string" ? (
        <p className="error" role="alert">
          {`rules: ${rules}`}
        </p>
      ) : null}
      <label>
        Find a rule
        <input autoFocus value={search} placeholder="sql, idor, CWE-79, INPV-05…" onChange={(ev) => setSearch(ev.target.value)} />
      </label>
      <label>
        Rule
        <select aria-label="Rule" size={6} value={rule} onChange={(ev) => setRule(ev.target.value)} required>
          {shown.map((r) => (
            <option key={r.id} value={r.id}>
              {ruleLine(r)}
            </option>
          ))}
        </select>
      </label>
      {chosen ? <p className="note wrap">{chosen.help ? `${ruleLine(chosen)} · ${chosen.help}` : ruleLine(chosen)}</p> : null}
      {rule === OTHER ? (
        <label>
          CWE (optional)
          <input value={cwe} inputMode="numeric" placeholder="693" onChange={(ev) => setCwe(ev.target.value)} />
        </label>
      ) : null}
      <div className="grid2">
        <label>
          Parameter
          <input
            aria-label="Parameter"
            value={parameter}
            list={`params-${entry.id}`}
            placeholder="optional; part of the fingerprint"
            onChange={(ev) => setParameter(ev.target.value)}
          />
        </label>
        <datalist id={`params-${entry.id}`}>
          {params.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
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
          <input value={rank} inputMode="numeric" placeholder="optional" onChange={(ev) => setRank(ev.target.value)} />
        </label>
      </div>
      <label>
        Message
        <textarea rows={2} value={message} placeholder="optional: what's wrong (defaults to the rule and endpoint)" onChange={(ev) => setMessage(ev.target.value)} />
      </label>
      {replays.length ? (
        <fieldset>
          <legend>Also as evidence</legend>
          {replays.map((r) => (
            <label key={r.id} className="check">
              <input
                type="checkbox"
                checked={picked.has(r.id)}
                onChange={(ev) =>
                  setPicked((prev) => {
                    const next = new Set(prev);
                    if (ev.target.checked) next.add(r.id);
                    else next.delete(r.id);
                    return next;
                  })
                }
              />
              {`replay ${shortId(r.id)} · ${r.status ?? "no response"}`}
            </label>
          ))}
        </fieldset>
      ) : null}
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
      <div className="row-actions">
        <button type="submit" disabled={busy || !rule}>
          {busy ? "flagging…" : "flag"} <kbd>Ctrl ⏎</kbd>
        </button>
        <button type="button" onClick={onCancel}>
          cancel <kbd>Esc</kbd>
        </button>
      </div>
    </form>
  );
}
