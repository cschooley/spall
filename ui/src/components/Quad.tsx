// Quad mode (#66): the countdown chip in the top bar, the screen-edge glow,
// and the scorecard when a burst ends, with its shareable card (numbers
// only, built by the server). No sound: the power-up sound is optional and
// off by default, and spall doesn't ship one.
import { useEffect, useState } from "react";
import { mmss, remaining, type Quad, type Scorecard } from "../lib/quad.ts";

export function QuadChip({ quad }: { quad: Quad }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return (
    <span className="quad-chip" role="status" title={`${quad.workers} agent workers at 4x rate${quad.capped ? `; ${quad.capped}` : ""}`}>
      {`QUAD ${mmss(remaining(quad, now))}`}
    </span>
  );
}

export function QuadGlow() {
  return <div className="quad-glow" aria-hidden="true" />;
}

export function QuadCard({ card, onClose }: { card: Scorecard; onClose(): void }) {
  const [copied, setCopied] = useState<"ok" | "failed" | null>(null);
  const copy = () =>
    navigator.clipboard.writeText(card.card).then(
      () => setCopied("ok"),
      () => setCopied("failed"),
    );
  return (
    <section className="quad-card" aria-label="Quad scorecard">
      <h3>{card.ended === "estop" ? "Quad: e-stopped" : card.ended === "stopped" ? "Quad: ended early" : "Quad: over"}</h3>
      <dl>
        <dt>time</dt>
        <dd>{mmss(card.seconds)}</dd>
        <dt>workers</dt>
        <dd>{card.workers}</dd>
        <dt>requests</dt>
        <dd>{card.requests}</dd>
        <dt>tokens</dt>
        <dd>{card.tokens}</dd>
        <dt>candidates</dt>
        <dd>{card.candidates}</dd>
      </dl>
      <p className="quad-share">
        <code>{card.card}</code>
        <span className="note"> · numbers only: no hosts, paths or names</span>
      </p>
      <div className="row">
        <button type="button" className="btn" onClick={() => void copy()}>
          {copied === "ok" ? "Copied" : copied === "failed" ? "Copy failed" : "Copy quad card"}
        </button>
        <button type="button" className="btn" onClick={onClose}>
          Close
        </button>
      </div>
    </section>
  );
}
