// Compare a replay with its original (#20): verdict, then response and
// request, changed headers and a body diff with unchanged runs collapsed.
// Built from lib/diff (pure); every value is attacker-controlled text.

import { useState } from "react";
import { collapse, compareExchanges, type BodyDiff, type HeaderRow } from "../lib/diff.ts";
import { humanSize, shortId, statusLine, timeOf } from "../lib/format.ts";
import type { TrafficEntry } from "../types.ts";

export function Compare({ original, replay, onOpen }: { original: TrafficEntry; replay: TrafficEntry; onOpen(id: string): void }) {
  const c = compareExchanges(original, replay);
  return (
    <div className="compare">
      <p className="verdict" role="status">
        {c.verdict}
      </p>
      <p className="note">
        {"original "}
        <button type="button" className="link" title={original.id} onClick={() => onOpen(original.id)}>
          {shortId(original.id)}
        </button>
        {` at ${timeOf(original.ts)} (${statusLine(original.status)}) → this replay at ${timeOf(replay.ts)}`}
      </p>
      <Section title="Response" headers={c.response.headers} body={c.response.body} />
      <Section title="Request" headers={c.request.headers} body={c.request.body} />
    </div>
  );
}

const KIND_LABEL: Record<HeaderRow["kind"], string> = {
  same: "same",
  added: "added",
  removed: "removed",
  changed: "changed",
  moved: "reordered",
};

function Section({ title, headers, body }: { title: string; headers: HeaderRow[]; body: BodyDiff }) {
  const changed = headers.filter((h) => h.kind !== "same");
  const same = headers.length - changed.length;
  return (
    <section aria-label={`${title} comparison`}>
      <h3>{title}</h3>
      <h4>
        Headers <span className="note">{` · ${changed.length ? `${changed.length} differ` : "no differences"}${same ? `, ${same} unchanged` : ""}`}</span>
      </h4>
      {changed.length ? (
        <dl className="hdiff">
          {changed.map((h) => (
            <div key={h.name} className={`hd hd-${h.kind}${h.volatile ? " volatile" : ""}`}>
              <dt>
                {h.name}
                <span className="tag">{KIND_LABEL[h.kind]}</span>
                {h.volatile ? <span className="tag" title="Changes on every response">volatile</span> : null}
              </dt>
              {h.a.length ? <dd className="was">{h.a.map((v, i) => <div key={i}>{`- ${v}`}</div>)}</dd> : null}
              {h.b.length ? <dd className="now">{h.b.map((v, i) => <div key={i}>{`+ ${v}`}</div>)}</dd> : null}
            </div>
          ))}
        </dl>
      ) : null}
      <h4>Body</h4>
      <BodyView d={body} />
    </section>
  );
}

function BodyView({ d }: { d: BodyDiff }) {
  const [expanded, setExpanded] = useState(false);
  if (d.kind === "binary" || d.kind === "too-large") {
    const what = d.kind === "binary" ? "Binary" : "Too large to diff line by line";
    return (
      <p className="note">{`${what}: ${d.same ? "identical" : "different"} (${humanSize(d.sizeA)} → ${humanSize(d.sizeB)})`}</p>
    );
  }
  if (d.lines.length === 0) return <p className="none">(no body on either side)</p>;
  if (d.changed === 0) return <p className="note">{`Identical (${d.lines.length} line${d.lines.length === 1 ? "" : "s"})`}</p>;
  const view = expanded ? d.lines : collapse(d.lines, 3);
  return (
    <div className="ldiff" role="group" aria-label="Body diff">
      {view.map((l, i) =>
        l.kind === "skip" ? (
          <button key={i} type="button" className="skip" onClick={() => setExpanded(true)}>
            {`… ${l.count} unchanged line${l.count === 1 ? "" : "s"} (show)`}
          </button>
        ) : (
          <div key={i} className={`l l-${l.kind}`}>
            {`${l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "} ${l.text}`}
          </div>
        ),
      )}
    </div>
  );
}
