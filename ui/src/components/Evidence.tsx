// A candidate's evidence (#45): the exchanges backing it (flux/replayIds),
// decisive one first, each opening in Traffic's inspector; a replay
// compared with its original using the same diff as the inspector (#20);
// and more exchanges added by id. Evidence is appended, never replaced
// (flux SPEC.md, "Evidence"). Every value is attacker-controlled text.

import { useEffect, useState } from "react";
import { Compare } from "./Compare.tsx";
import type { Candidate } from "../lib/candidates.ts";
import { displayUrl, shortId, statusClass, statusLine, timeOf } from "../lib/format.ts";
import type { TrafficEntry } from "../types.ts";

const ID = /^[A-Za-z0-9_-]{1,128}$/;

async function fetchEntry(id: string): Promise<TrafficEntry | null> {
  if (!ID.test(id)) return null;
  try {
    const res = await fetch(`/api/exchanges/${encodeURIComponent(id)}`);
    if (!res.ok) return null;
    const v = (await res.json()) as unknown;
    const o = v as Record<string, unknown> | null;
    return o && typeof o.id === "string" && typeof o.method === "string" && typeof o.url === "string" ? (v as TrafficEntry) : null;
  } catch {
    return null;
  }
}

export function Evidence({ candidate, onOpenExchange, onAdd }: { candidate: Candidate; onOpenExchange(id: string): void; onAdd(ids: string[]): void }) {
  const ids = candidate.exchanges;
  const [entries, setEntries] = useState<Record<string, TrafficEntry | null | undefined>>({});
  const [comparing, setComparing] = useState<{ replay: TrafficEntry; original: TrafficEntry | null } | null>(null);
  const [adding, setAdding] = useState("");
  const [err, setErr] = useState<string | null>(null);

  const key = ids.join(",");
  useEffect(() => {
    let live = true;
    void Promise.all(ids.map(async (id) => [id, await fetchEntry(id)] as const)).then((pairs) => live && setEntries(Object.fromEntries(pairs)));
    return () => {
      live = false;
    };
    // `key` stands for `ids`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const compare = async (replay: TrafficEntry) => {
    setComparing({ replay, original: replay.replay_of ? await fetchEntry(replay.replay_of) : null });
  };

  const add = () => {
    const want = adding.split(/[\s,]+/).filter(Boolean);
    if (!want.length) return;
    const bad = want.find((x) => !ID.test(x));
    if (bad) return setErr(`not an exchange id: ${bad}`);
    setErr(null);
    setAdding("");
    onAdd(want);
  };

  return (
    <div className="evidence" role="group" aria-label="Evidence">
      <h3>
        Evidence <span className="note">{`· ${ids.length} exchange${ids.length === 1 ? "" : "s"}, decisive first; added to, never replaced`}</span>
      </h3>
      {candidate.evidence ? (
        <p className="decisive">
          {`${candidate.evidence.method ?? ""} ${candidate.evidence.target ?? ""} → ${candidate.evidence.noResponse ? "no response" : statusLine(candidate.evidence.status)}`}
        </p>
      ) : null}
      <ol className="ev-list" aria-label="Evidence exchanges">
        {ids.map((id, i) => {
          const e = entries[id];
          return (
            <li key={id}>
              <button type="button" className="link" title={`Open ${id} in Traffic`} onClick={() => onOpenExchange(id)}>
                {shortId(id)}
              </button>
              {i === 0 ? <span className="tag">decisive</span> : null}
              {e === undefined ? <span className="note">…</span> : null}
              {e === null ? <span className="note">not in taphttp's history</span> : null}
              {e ? (
                <>
                  <span className="t">{timeOf(e.ts)}</span>
                  <span>{`${e.method} ${displayUrl(e.url)}`}</span>
                  <span className={statusClass(e.status)}>{statusLine(e.status)}</span>
                  {e.replay_of ? (
                    <button type="button" className="cmp" onClick={() => void compare(e)}>
                      Compare
                    </button>
                  ) : null}
                </>
              ) : null}
            </li>
          );
        })}
      </ol>
      {comparing ? (
        comparing.original ? (
          <Compare original={comparing.original} replay={comparing.replay} onOpen={onOpenExchange} />
        ) : (
          <p className="none">{`The original (${comparing.replay.replay_of ?? "?"}) isn't in taphttp's history.`}</p>
        )
      ) : null}
      <div className="ev-add">
        <input
          aria-label="Add evidence"
          value={adding}
          placeholder="exchange id(s) to add as evidence"
          onChange={(ev) => setAdding(ev.target.value)}
          onKeyDown={(ev) => {
            if (ev.key === "Enter") {
              ev.preventDefault();
              add();
            }
          }}
        />
        <button type="button" onClick={add} disabled={!adding.trim()}>
          add
        </button>
      </div>
      {err ? (
        <p className="error" role="alert">
          {err}
        </p>
      ) : null}
    </div>
  );
}
