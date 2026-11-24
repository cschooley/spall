// The intake panel (#65): attached to the active engagement in Tasks. The
// tester says what they want; the orchestrator answers with a draft plan
// (tasks with their costs, warnings, a pick list, drafts only they can act
// on) or "no." Nothing runs until they approve the plan, with two presses.
// The request, warnings and goals can carry captured text: text only.
import { useEffect, useState } from "react";
import { askIntake, changePlan, draftPlans, type Answer, type Plan } from "../lib/intake.ts";

export function IntakePanel({ agentModel, onMade, onClose }: { agentModel: string | null; onMade(ids: string[]): void; onClose(): void }) {
  const [text, setText] = useState("");
  const [asked, setAsked] = useState("");
  const [busy, setBusy] = useState(false);
  const [answer, setAnswer] = useState<Answer | string | null>(null);
  const [drafts, setDrafts] = useState<Plan[]>([]);
  useEffect(() => {
    let live = true;
    void draftPlans().then((p) => live && Array.isArray(p) && setDrafts(p));
    return () => {
      live = false;
    };
  }, [answer]);
  const ask = async (words: string, pick?: { question: string; answer: string }) => {
    setBusy(true);
    setAsked(words);
    const a = await askIntake(words, pick);
    setBusy(false);
    setAnswer(a);
  };
  const shown = answer && typeof answer !== "string" && answer.kind === "plan" ? answer.plan : null;
  return (
    <section className="work intake" aria-label="Intake">
      <div className="list-head">
        <h2>Ask for work</h2>
        <span className="spacer" />
        <button type="button" className="btn" onClick={onClose}>
          Close
        </button>
      </div>
      <p className="note">
        Say what you want done in this engagement. The intake plans tasks with their costs; nothing runs until you approve the plan.
        {agentModel ? ` (${agentModel})` : ""}
      </p>
      {agentModel ? null : <p className="error">The intake needs spall's model (--model-provider and a key). The agent buttons and + New task work without one.</p>}
      <form
        className="intake-ask"
        onSubmit={(e) => {
          e.preventDefault();
          if (text.trim()) void ask(text.trim());
        }}
      >
        <textarea aria-label="Ask the intake" rows={3} maxLength={2000} value={text} placeholder="re-scan the findings from last week's test" onChange={(e) => setText(e.target.value)} />
        <button type="submit" className="btn primary" disabled={busy || !text.trim() || !agentModel}>
          {busy ? "Planning…" : "Plan it"}
        </button>
      </form>
      {typeof answer === "string" ? <p className="error">{answer}</p> : null}
      {answer && typeof answer !== "string" && answer.kind === "no" ? (
        <p className="intake-no" role="status">
          no.
        </p>
      ) : null}
      {answer && typeof answer !== "string" && answer.kind === "said" ? <p role="status">{answer.text}</p> : null}
      {shown ? (
        <PlanCard
          key={shown.id}
          plan={shown}
          busy={busy}
          onPick={(option) => shown.choice && void ask(asked || shown.request, { question: shown.choice.question, answer: option })}
          onChanged={(p) => setAnswer({ kind: "plan", plan: p })}
          onMade={onMade}
        />
      ) : null}
      {drafts.filter((p) => p.id !== shown?.id).length ? (
        <section className="sect" aria-label="Draft plans">
          <h3>Draft plans</h3>
          <ul className="plan-drafts">
            {drafts
              .filter((p) => p.id !== shown?.id)
              .map((p) => (
                <li key={p.id}>
                  <button type="button" className="link" onClick={() => setAnswer({ kind: "plan", plan: p })}>
                    {p.id}
                  </button>
                  {` · ${p.estimate} · ${p.request}`}
                </li>
              ))}
          </ul>
        </section>
      ) : null}
    </section>
  );
}

function PlanCard({ plan, busy, onPick, onChanged, onMade }: { plan: Plan; busy: boolean; onPick(option: string): void; onChanged(p: Plan): void; onMade(ids: string[]): void }) {
  const [armed, setArmed] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const act = async (op: Parameters<typeof changePlan>[1]) => {
    setErr(null);
    const r = await changePlan(plan.id, op);
    if (typeof r === "string") return setErr(r);
    onChanged(r);
    if (op.op === "approve") onMade(r.made);
  };
  const draft = plan.status === "draft";
  return (
    <section className="sect plan" aria-label={`Plan ${plan.id}`}>
      <h3>
        {`${plan.id} · ${plan.status}`} <span className="note">{`· for ${plan.engagement}`}</span>
      </h3>
      <p className="note">{`You asked: ${plan.request}`}</p>
      {plan.summary ? <p>{plan.summary}</p> : null}
      {plan.warnings.length ? (
        <ul className="plan-warnings" aria-label="Warnings">
          {plan.warnings.map((w) => (
            <li key={w}>{`⚠ ${w}`}</li>
          ))}
        </ul>
      ) : null}
      {plan.proposal ? (
        <p className="plan-proposal">
          {plan.proposal.kind === "extension"
            ? `Drafted for you: extend ${plan.proposal.engagement} (${plan.proposal.why}). Only you change an engagement: `
            : `Drafted for you: an engagement "${plan.proposal.name}" with scope ${plan.proposal.scope.join(", ")} (${plan.proposal.why}). Only you create one: `}
          <a href="#engagement">open Engagement</a>
        </p>
      ) : null}
      {plan.choice ? (
        <div className="plan-choice" role="group" aria-label={plan.choice.question}>
          <p>{plan.choice.question}</p>
          {plan.choice.options.map((o) => (
            <button key={o} type="button" className="btn" disabled={busy} onClick={() => onPick(o)}>
              {o}
            </button>
          ))}
        </div>
      ) : null}
      {plan.tasks.length ? (
        <>
          <table className="plan-tasks">
            <thead>
              <tr>
                <th>Role</th>
                <th>Task</th>
                <th>Cost</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {plan.tasks.map((t, i) => (
                <tr key={`${i}-${t.goal}`}>
                  <td>{t.role}</td>
                  <td>
                    {t.goal}
                    {t.candidate ? (
                      <>
                        {" · "}
                        <a href={`#findings/${encodeURIComponent(t.candidate)}`}>{t.candidate}</a>
                      </>
                    ) : null}
                  </td>
                  <td className="note">{t.cost}</td>
                  <td>
                    {draft ? (
                      <button type="button" className="hint" aria-label={`Leave out task ${i + 1}`} onClick={() => void act({ op: "drop", index: i })}>
                        leave out
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="plan-estimate">{plan.estimate}</p>
        </>
      ) : null}
      {draft ? (
        <div className="row">
          {plan.tasks.length ? (
            <button
              type="button"
              className={armed ? "btn primary armed" : "btn primary"}
              onClick={() => {
                if (!armed) return setArmed(true);
                setArmed(false);
                void act({ op: "approve" });
              }}
              onBlur={() => setArmed(false)}
            >
              {armed ? `Press again to make and approve ${plan.tasks.length} task${plan.tasks.length === 1 ? "" : "s"}` : "Approve plan"}
            </button>
          ) : null}
          <button type="button" className="btn" onClick={() => void act({ op: "discard" })}>
            Discard
          </button>
        </div>
      ) : null}
      {plan.status === "approved" ? (
        <p role="status">
          {"Made and approved: "}
          {plan.made.map((id, i) => (
            <span key={id}>
              {i ? ", " : ""}
              <a href={`#tasks/${id}`}>{id}</a>
            </span>
          ))}
        </p>
      ) : null}
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
    </section>
  );
}
