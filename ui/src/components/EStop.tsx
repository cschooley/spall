// The agent e-stop (#57, docs/ux.md): always in the top bar. One press (or
// Shift+Esc) halts every agent task for good; clearing it takes two
// presses and only lets new tasks run. Visual target: verdigris
// mockups/spall-tasks.html (.estop, .halt-banner). Text from the server
// is rendered as text.
import { useEffect, useState } from "react";
import { sinceText, type EStop } from "../lib/estop.ts";

export function EStopButton({
  estop,
  available,
  onEngage,
  onClear,
}: {
  estop: EStop | null;
  available: boolean;
  onEngage: () => void;
  onClear: () => void;
}) {
  // Armed for one particular stop (its `since`): a stop engaged anew is
  // never cleared by a press meant for an earlier one.
  const [armedFor, setArmedFor] = useState<string | null>(null);
  useEffect(() => {
    if (armedFor === null) return;
    const t = setTimeout(() => setArmedFor(null), 4000);
    return () => clearTimeout(t);
  }, [armedFor]);
  const armed = estop !== null && armedFor === estop.since;
  if (!estop) {
    return (
      <button
        type="button"
        className="estop"
        disabled={!available}
        title={available ? "Halt every agent task, for good (Shift+Esc)" : "Agents need taphttp's lever API"}
        onClick={onEngage}
      >
        <i aria-hidden="true" />
        STOP AGENTS
      </button>
    );
  }
  return (
    <button
      type="button"
      className={`estop on${armed ? " armed" : ""}`}
      title="Agents are stopped. Press twice to clear: new tasks may run; halted tasks never do."
      onClick={() => {
        if (armed) {
          setArmedFor(null);
          onClear();
        } else setArmedFor(estop.since);
      }}
    >
      <i aria-hidden="true" />
      {armed ? "PRESS AGAIN TO CLEAR" : "AGENTS STOPPED"}
    </button>
  );
}

export function HaltBanner({ estop }: { estop: EStop }) {
  return (
    <div className="halt-banner" role="alert">
      <b>■ ALL AGENT TASKS HALTED</b>
      <span>
        since {sinceText(estop.since)}
        {estop.reason ? ` · ${estop.reason}` : ""}
      </span>
      <span className="spacer" />
      <span>No agent requests are sent, and halted tasks never run again. Human traffic is unaffected.</span>
    </div>
  );
}
