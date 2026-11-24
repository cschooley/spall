// The Tasks view (#61, key 4): the task list, the open task's card (goal,
// constraints, plan, proposed candidates, request log) and a steering
// panel recorded in the task's log. Visual target: verdigris
// mockups/spall-tasks.html. Goals, targets, reasons and model text are an
// agent's words or captured traffic: rendered as text only. The view polls
// while it's open: the SSE stream carries traffic only.
import { useCallback, useEffect, useState } from "react";
import { apiJson } from "../lib/engagement.ts";
import { asCandidates, type Candidate } from "../lib/candidates.ts";
import {
  asTask,
  asTasks,
  asWaiting,
  changeTask,
  chipOf,
  createTask,
  finished,
  messages,
  requestRows,
  taskFromHash,
  usageText,
  type Task,
} from "../lib/tasks.ts";
import { Card } from "./Approvals.tsx";

const POLL_MS = 2000;

function Chip({ status }: { status: string }) {
  const c = chipOf(status);
  return (
    <span className={`chip ${c.cls}`}>
      {c.cls === "running" ? <span className="dot" aria-hidden="true" /> : null}
      {c.label}
    </span>
  );
}

function Budget({ spent, budget }: { spent: number; budget: number }) {
  const pct = budget > 0 ? Math.min(100, (spent / budget) * 100) : 0;
  return (
    <span className="budget" aria-label={`${spent} of ${budget} requests spent`}>
      <span className="track">
        <span className="fill" style={{ width: `${pct}%` }} />
      </span>
      <b>{spent}</b>
      {`/${budget} req`}
    </span>
  );
}

/** Who runs a task, in words. */
function runBy(t: Task, agentModel: string | null): string {
  if (t.runner === "external") return "run by an MCP client";
  return agentModel ? `run by spall (${agentModel})` : "run by spall (no model configured)";
}

export function TasksView({
  available,
  agentModel,
  onChanged,
  onOpenExchange,
}: {
  available: boolean;
  /** The model spall's own host runs (#63); null without one. */
  agentModel: string | null;
  onChanged(): void;
  onOpenExchange(id: string): void;
}) {
  const [list, setList] = useState<Task[] | string | null>(null);
  const [sel, setSel] = useState<string | null>(() => taskFromHash(window.location.hash));
  const [open, setOpen] = useState<Task | string | null>(null);
  const [creating, setCreating] = useState(false);
  const [nonce, setNonce] = useState(0);
  const refresh = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    const onHash = () => setSel(taskFromHash(window.location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (!available) return;
    let live = true;
    const poll = async () => {
      const body = await apiJson("/api/tasks");
      if (!live) return;
      setList(typeof body === "string" ? body : asTasks(body));
      if (sel) {
        const t = await apiJson(`/api/tasks/${encodeURIComponent(sel)}`);
        if (live) setOpen(typeof t === "string" ? t : (asTask(t) ?? "unexpected answer from spall"));
      }
    };
    void poll();
    const id = setInterval(() => void poll(), POLL_MS);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [available, sel, nonce]);

  if (!available) {
    return <p className="empty">Tasks need taphttp's lever API (start spall with --taphttp-api).</p>;
  }
  const tasks = Array.isArray(list) ? list : [];
  const done = () => {
    refresh();
    onChanged();
  };
  return (
    <div className="tasks">
      <section className="list" aria-label="Tasks">
        <div className="list-head">
          <h1>Tasks</h1>
          <span className="spacer" />
          <button type="button" className="btn primary" onClick={() => setCreating(true)}>
            + New task
          </button>
        </div>
        {typeof list === "string" ? <p className="error">{list}</p> : null}
        {Array.isArray(list) && tasks.length === 0 ? (
          <p className="note pad">No tasks yet. Make one here, or connect an MCP client: its first session makes a draft for you to approve.</p>
        ) : null}
        <ol className="task-list">
          {tasks.map((t) => (
            <li key={t.id}>
              <a
                href={`#tasks/${encodeURIComponent(t.id)}`}
                className={`task${t.id === sel ? " sel" : ""}${finished(t.status) ? " done" : ""}`}
                aria-current={t.id === sel ? "true" : undefined}
                onClick={() => setCreating(false)}
              >
                <span className="row1">
                  <span className="id">{t.id}</span>
                  <Chip status={t.status} />
                  <span className="note">{t.runner === "spall" ? `${t.role} · ◆ spall` : t.role}</span>
                </span>
                <span className="name">{t.goal}</span>
                <Budget spent={t.spent} budget={t.budget} />
              </a>
            </li>
          ))}
        </ol>
      </section>
      {creating ? (
        <NewTaskForm
          agentModel={agentModel}
          onCancel={() => setCreating(false)}
          onCreated={(id) => {
            setCreating(false);
            window.location.hash = `tasks/${id}`;
            done();
          }}
        />
      ) : sel && open && typeof open !== "string" && open.id === sel ? (
        <>
          <TaskCard t={open} agentModel={agentModel} onChanged={done} onOpenExchange={onOpenExchange} />
          <Steer t={open} onChanged={done} />
        </>
      ) : (
        <section className="work empty-work" aria-label="Task detail">
          {typeof open === "string" && sel ? <p className="error">{open}</p> : <p className="note">Pick a task to see its plan, request log and what it waits on.</p>}
        </section>
      )}
    </div>
  );
}

function TaskCard({ t, agentModel, onChanged, onOpenExchange }: { t: Task; agentModel: string | null; onChanged(): void; onOpenExchange(id: string): void }) {
  const [armed, setArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cands, setCands] = useState<Candidate[]>([]);
  useEffect(() => {
    if (!armed) return;
    const id = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(id);
  }, [armed]);
  useEffect(() => {
    let live = true;
    void apiJson("/api/candidates").then((b) => live && typeof b !== "string" && setCands(asCandidates(b).filter((c) => c.flaggedBy === `agent:${t.id}`)));
    return () => {
      live = false;
    };
  }, [t.id, t.logLen]);
  const act = async (op: Record<string, unknown>) => {
    const err = await changeTask(t.id, op);
    setError(err);
    if (!err) onChanged();
  };
  const waiting = asWaiting({ tasks: [rawOf(t)] })[0];
  const rows = requestRows(t);
  const list = (l: string[], empty: string) => (l.length ? l.join(", ") : empty);
  return (
    <section className="work" aria-label="Task detail">
      <header>
        <div className="row1">
          <span className="id">{t.id}</span>
          <Chip status={t.status} />
          <span className="note">{`${t.role} · asked by ${t.askedBy} · ${runBy(t, agentModel)}`}</span>
          <span className="spacer" />
          {t.status === "draft" ? (
            <button type="button" className="btn primary" onClick={() => void act({ op: "approve" })}>
              Approve
            </button>
          ) : null}
          {!finished(t.status) ? (
            <button
              type="button"
              className={`btn halt${armed ? " armed" : ""}`}
              title="Stops it for good: a halted task never runs again"
              onClick={() => (armed ? (setArmed(false), void act({ op: "halt" })) : setArmed(true))}
            >
              {armed ? "press again: halt for good" : "Halt"}
            </button>
          ) : null}
        </div>
        <h2>{t.goal}</h2>
        {t.reason ? <p className="reason">{t.reason}</p> : null}
        {error ? (
          <p className="error" role="alert">
            {error}
          </p>
        ) : null}
      </header>
      {waiting && t.status === "waiting_on_approval" ? (
        <div className="sect">
          <h3>Waiting on you</h3>
          <Card key={waiting.pending.asked} w={waiting} onDone={onChanged} />
        </div>
      ) : null}
      <div className="cols">
        <div className="sect">
          <h3>Constraints</h3>
          <dl>
            <dt>hosts</dt>
            <dd>{list(t.hosts, "the engagement's scope")}</dd>
            <dt>methods</dt>
            <dd>{list(t.methods, "any the rules allow")}</dd>
            <dt>no asking</dt>
            <dd>{list(t.safeMethods, "none: every send asks")}</dd>
            <dt>rate</dt>
            <dd>{t.rate === null ? "no ceiling" : `≤ ${t.rate} req/s`}</dd>
            <dt>stop if</dt>
            <dd>
              {[t.stop.serverErrors ? `${t.stop.serverErrors} × 5xx` : null, t.stop.throttled ? `${t.stop.throttled} × 429` : null].filter(Boolean).join(" · ") ||
                "no stop condition"}
            </dd>
            <dt>time</dt>
            <dd>{`${t.timeLimit} min${t.started ? ` · started ${t.started.slice(11, 19)}` : ""}`}</dd>
            <dt>budget</dt>
            <dd>
              <Budget spent={t.spent} budget={t.budget} />
            </dd>
            {t.runner === "spall" ? (
              <>
                <dt>model</dt>
                <dd data-testid="model-usage">{usageText(t.usage)}</dd>
              </>
            ) : null}
          </dl>
        </div>
        <div className="sect">
          <h3>Plan</h3>
          {t.plan.length ? (
            <ol className="plan">
              {t.plan.map((p, i) => (
                <li key={i} className={p.done ? "done" : undefined}>
                  <span className="mk" aria-hidden="true">
                    {p.done ? "✓" : "○"}
                  </span>
                  {p.text}
                </li>
              ))}
            </ol>
          ) : (
            <p className="note">No plan yet.</p>
          )}
        </div>
      </div>
      <div className="sect">
        <h3>
          Proposed candidates <span className="note">· you decide in Findings</span>
        </h3>
        {cands.length ? (
          <ul className="cands">
            {cands.map((c) => (
              <li key={c.id}>
                <a href={`#findings/${encodeURIComponent(c.id)}`}>{`${c.id} · ${c.ruleId}`}</a>
                <span className="note">{c.message}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="note">None yet.</p>
        )}
      </div>
      <div className="sect">
        <h3>
          Request log <span className="note">· every sent row opens in Traffic</span>
        </h3>
        {rows.length ? (
          <div className="logwrap">
            <table aria-label={`${t.id} request log`}>
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Method</th>
                  <th>Target</th>
                  <th>Status</th>
                  <th>Outcome</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r, i) => (
                  <tr key={i}>
                    <td className="t">{r.time}</td>
                    <td>{r.method}</td>
                    <td className="target">
                      {r.exchange ? (
                        <button type="button" className="link" title={`Open ${r.exchange} in Traffic`} onClick={() => onOpenExchange(r.exchange!)}>
                          {r.target}
                        </button>
                      ) : (
                        r.target
                      )}
                    </td>
                    <td>{r.status ?? ""}</td>
                    <td className="note">{r.outcome}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="note">No requests yet.</p>
        )}
      </div>
    </section>
  );
}

/** The fields asWaiting reads, back in the server's shape. */
function rawOf(t: Task): Record<string, unknown> {
  return { id: t.id, goal: t.goal, role: t.role, status: t.status, spent: t.spent, constraints: { budget: t.budget, time_limit_minutes: t.timeLimit }, pending: t.pendingRaw };
}

function Steer({ t, onChanged }: { t: Task; onChanged(): void }) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const msgs = messages(t);
  const send = async () => {
    const err = await changeTask(t.id, { op: "note", text });
    setError(err);
    if (!err) {
      setText("");
      onChanged();
    }
  };
  return (
    <aside className="steer" aria-label={`Steer ${t.id}`}>
      <h3>{`Steer ${t.id}`}</h3>
      <div className="msgs">
        {msgs.length === 0 ? <p className="note">Nothing yet.</p> : null}
        {msgs.map((m, i) => (
          <div key={i} className={`msg ${m.who}`}>
            {m.who === "sys" ? (
              `${m.time} ${m.text}`
            ) : (
              <>
                <span className="who">{m.who === "agent" ? `◆ agent · ${m.time}` : `you · ${m.time}`}</span>
                {m.text}
              </>
            )}
          </div>
        ))}
      </div>
      {finished(t.status) ? (
        <p className="note compose">{`${t.id} is ${t.status}: nothing more runs under it.`}</p>
      ) : (
        <form
          className="compose"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <textarea aria-label={`Steer ${t.id}`} placeholder="Steer the agent… e.g. focus on the remember-me cookie next" value={text} onChange={(e) => setText(e.target.value)} />
          <button type="submit" className="btn" disabled={!text.trim()}>
            Send
          </button>
          {error ? (
            <p className="error" role="alert">
              {error}
            </p>
          ) : null}
          <p className="hint">
            The agent reads this with its task. It proposes findings; only you promote them. <kbd>Shift</kbd>+<kbd>Esc</kbd> stops every task.
          </p>
        </form>
      )}
    </aside>
  );
}

function NewTaskForm({ agentModel, onCancel, onCreated }: { agentModel: string | null; onCancel(): void; onCreated(id: string): void }) {
  const [goal, setGoal] = useState("");
  const [bySpall, setBySpall] = useState(false);
  const [tokens, setTokens] = useState("2000000");
  const [role, setRole] = useState("mapper");
  const [budget, setBudget] = useState("100");
  const [time, setTime] = useState("60");
  const [hosts, setHosts] = useState("");
  const [methods, setMethods] = useState("");
  const [note, setNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  const split = (s: string) =>
    s
      .split(/[,\s]+/)
      .map((x) => x.trim())
      .filter(Boolean);
  const submit = async () => {
    const n = (s: string) => (/^\d+$/.test(s.trim()) ? Number(s) : undefined);
    const spall = bySpall && agentModel !== null;
    const r = await createTask({
      goal,
      role,
      budget: n(budget),
      timeLimit: n(time),
      hosts: split(hosts),
      methods: split(methods).map((m) => m.toUpperCase()),
      note,
      ...(spall ? { runner: "spall" as const, tokens: n(tokens) } : {}),
    });
    if (typeof r === "string") setError(r);
    else onCreated(r.id);
  };
  return (
    <section className="work" aria-label="New task">
      <form
        className="new-task"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h2>New task</h2>
        <p className="note">A draft: nothing runs until you approve it. Constraints only narrow the rules of engagement; anything wider is refused.</p>
        <label>
          Goal
          <textarea autoFocus value={goal} onChange={(e) => setGoal(e.target.value)} placeholder="Map the /v1 API from traffic and replay its GETs" />
        </label>
        <fieldset className="run-by">
          <legend>Run by</legend>
          <label>
            <input type="radio" name="run-by" checked={!bySpall || agentModel === null} onChange={() => setBySpall(false)} />
            an MCP client <span className="note">(Claude Code, Claude Desktop, … connects to this task)</span>
          </label>
          <label>
            <input type="radio" name="run-by" disabled={agentModel === null} checked={bySpall && agentModel !== null} onChange={() => setBySpall(true)} />
            {agentModel ? (
              <>
                spall <span className="note">{`(${agentModel}, with your key; starts when you approve)`}</span>
              </>
            ) : (
              <>
                spall <span className="note">(no model configured: start spall with --model-provider)</span>
              </>
            )}
          </label>
          {bySpall && agentModel !== null ? (
            <label>
              Model tokens <span className="note">(in and out; past this it halts)</span>
              <input inputMode="numeric" value={tokens} onChange={(e) => setTokens(e.target.value)} />
            </label>
          ) : null}
        </fieldset>
        <div className="row">
          <label>
            Role
            <select value={role} onChange={(e) => setRole(e.target.value)}>
              {["scoper", "mapper", "tester", "reporter"].map((r) => (
                <option key={r}>{r}</option>
              ))}
            </select>
          </label>
          <label>
            Budget (requests)
            <input inputMode="numeric" value={budget} onChange={(e) => setBudget(e.target.value)} />
          </label>
          <label>
            Time limit (min)
            <input inputMode="numeric" value={time} onChange={(e) => setTime(e.target.value)} />
          </label>
        </div>
        <label>
          Hosts <span className="note">(blank: the engagement's scope)</span>
          <input value={hosts} onChange={(e) => setHosts(e.target.value)} placeholder="api.example.test, app.example.test" />
        </label>
        <label>
          Methods <span className="note">(blank: any the rules allow)</span>
          <input value={methods} onChange={(e) => setMethods(e.target.value)} placeholder="GET, HEAD" />
        </label>
        <label>
          Note <span className="note">(who asked, and why)</span>
          <input value={note} onChange={(e) => setNote(e.target.value)} />
        </label>
        {error ? (
          <p className="error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="actions">
          <button type="submit" className="btn primary" disabled={!goal.trim()}>
            Create draft
          </button>
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </form>
    </section>
  );
}
