// Agent buttons (#67): verbs on the exchange or candidate they sit on. Each
// drafts a pre-filled task (role, scope, budget, which sends ask) and says
// its cost first; the task runs only when it's approved in Tasks. The same
// verbs are palette entries, console commands and MCP tools.
import { useState } from "react";
import { CATEGORIES, runVerb, verbsOn, type VerbOn } from "../lib/verbs.ts";

export function AgentVerbs({ on, id, evidence = 0, available }: { on: VerbOn; id: string; evidence?: number; available: boolean }) {
  const [busy, setBusy] = useState<string | null>(null);
  const [category, setCategory] = useState(CATEGORIES[0]![0]);
  const [msg, setMsg] = useState<{ ok: true; task: string; text: string } | { ok: false; text: string } | null>(null);
  const run = async (name: string) => {
    setBusy(name);
    const r = await runVerb(name, id, name === "test_for" ? category : undefined);
    setBusy(null);
    setMsg(typeof r === "string" ? { ok: false, text: `${name}: ${r}` } : { ok: true, task: r.id, text: `(${r.role}, ${r.cost}): approve it in Tasks to run it.` });
  };
  return (
    <section className="verbs" aria-label="Agent">
      <h3>
        ◆ Agent <span className="note">· drafts a task; nothing runs until you approve it</span>
      </h3>
      <div className="verb-row">
        {verbsOn(on).map((v) => (
          <span key={v.name} className="verb">
            {v.name === "test_for" ? (
              <select aria-label="WSTG category" value={category} onChange={(e) => setCategory(e.target.value)}>
                {CATEGORIES.map(([c, name]) => (
                  <option key={c} value={c} title={name}>
                    {c}
                  </option>
                ))}
              </select>
            ) : null}
            <button type="button" className="btn" data-verb={v.name} disabled={!available || busy !== null} title={`${v.role}: ${v.hint(evidence)}`} onClick={() => void run(v.name)}>
              {busy === v.name ? "Drafting…" : v.label}
            </button>
            <span className="cost">{v.hint(evidence)}</span>
          </span>
        ))}
      </div>
      {msg ? (
        <p className={msg.ok ? "verb-msg" : "verb-msg error"} role="status">
          {msg.ok ? (
            <>
              Drafted <a href={`#tasks/${msg.task}`}>{msg.task}</a> {msg.text}
            </>
          ) : (
            msg.text
          )}
        </p>
      ) : null}
    </section>
  );
}
