// Tasks waiting on approval (#56), in the Engagement view's "Waiting on
// you". Each card shows exactly what would be sent, why it needs a human,
// and what each answer does. Targets, Host headers, goals and reasons are
// captured traffic or an agent's words: text only.
import { useEffect, useState } from "react";
import { costText, decide, forTaskText, listWaiting, type Decision, type Waiting } from "../lib/tasks.ts";

export function Approvals({ count, onChanged }: { count: number | null; onChanged(): void }) {
  const [list, setList] = useState<Waiting[] | string | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let live = true;
    void listWaiting().then((l) => live && setList(l));
    return () => {
      live = false;
    };
  }, [count, nonce]);
  const waiting = Array.isArray(list) ? list : [];
  return (
    <div className="waiting approvals" role="group" aria-label="Tasks waiting on approval">
      <h4>{`Tasks waiting on approval · ${waiting.length}`}</h4>
      {typeof list === "string" ? <p className="error">{list}</p> : null}
      {Array.isArray(list) && waiting.length === 0 ? <p className="note">No task is waiting on you.</p> : null}
      {waiting.map((w) => (
        <Card
          key={`${w.id}/${w.pending.asked}`}
          w={w}
          onDone={() => {
            setNonce((n) => n + 1);
            onChanged();
          }}
        />
      ))}
    </div>
  );
}

export function Card({ w, onDone }: { w: Waiting; onDone(): void }) {
  const [denying, setDenying] = useState(false);
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const forTask = forTaskText(w);
  const go = async (d: Decision) => {
    setBusy(true);
    const err = await decide(w.id, d);
    setBusy(false);
    if (err) setError(err);
    else onDone();
  };
  const p = w.pending;
  return (
    <article className="approval" aria-label={`Approval for ${w.id}`}>
      <header>
        <b>{w.id}</b>
        <span className="note">{w.role}</span>
        <span className="goal">{w.goal}</span>
      </header>
      <dl className="kv">
        <dt>request</dt>
        <dd className="req">
          <span className="method">{p.method}</span> {p.target}
        </dd>
        {p.hostHeader !== null ? (
          <>
            <dt>Host</dt>
            <dd>{p.hostHeader}</dd>
          </>
        ) : null}
        <dt>why</dt>
        <dd>{p.reason}</dd>
        <dt>cost</dt>
        <dd>{costText(w)}</dd>
      </dl>
      <div className="row-actions">
        <button type="button" disabled={busy} onClick={() => void go({ op: "approve_once" })} title="Send exactly this request, once, within 10 minutes">
          Approve once
        </button>
        <button
          type="button"
          disabled={busy || forTask === null}
          onClick={() => void go({ op: "approve_for_task" })}
          title={forTask === null ? "The rules of engagement want this confirmed each time: only once" : "Within the rules of engagement"}
        >
          {forTask === null ? "Approve for the task" : `Approve for the task: ${forTask}`}
        </button>
        {!denying ? (
          <button type="button" disabled={busy} onClick={() => setDenying(true)}>
            Deny…
          </button>
        ) : null}
      </div>
      {denying ? (
        <form
          className="deny"
          onSubmit={(e) => {
            e.preventDefault();
            void go({ op: "deny", reason });
          }}
        >
          <input autoFocus aria-label={`Why deny ${w.id}'s request`} placeholder="why (the agent reads this and tries another way)" value={reason} onChange={(e) => setReason(e.target.value)} />
          <button type="submit" disabled={busy || !reason.trim()}>
            Deny
          </button>
          <button type="button" onClick={() => setDenying(false)}>
            cancel
          </button>
        </form>
      ) : null}
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}
    </article>
  );
}
