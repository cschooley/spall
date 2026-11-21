import { Fragment, memo, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Detail } from "./components/Detail.tsx";
import {
  durationText,
  matchesFilter,
  pathOf,
  statusClass,
  statusText,
  timeOf,
} from "./lib/format.ts";
import type { TrafficEntry } from "./types.ts";
import { useTraffic, type Connection } from "./useTraffic.ts";

const CONNECTION_LABEL: Record<Connection, string> = {
  connecting: "connecting",
  live: "live",
  reconnecting: "reconnecting",
  down: "disconnected",
};

export function App() {
  const { store, connection, clear } = useTraffic();
  const [filter, setFilter] = useState("");
  const [follow, setFollow] = useState(true);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const scroller = useRef<HTMLElement>(null);
  const programmaticScroll = useRef(false);

  const q = filter.trim().toLowerCase();
  const rows = useMemo(() => store.items.filter((e) => matchesFilter(e, q)), [store.items, q]);

  const toggle = useCallback((id: string) => {
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }, []);

  // Follow the tail while following is on.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!follow || !el) return;
    programmaticScroll.current = true;
    el.scrollTop = el.scrollHeight;
    requestAnimationFrame(() => {
      programmaticScroll.current = false;
    });
  }, [rows.length, follow]);

  // Scrolling up stops following; scrolling back to the bottom resumes it.
  const onScroll = () => {
    const el = scroller.current;
    if (!el || programmaticScroll.current) return;
    setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
  };

  const count = store.items.length;
  return (
    <>
      <header>
        <h1>spall</h1>
        <span className={`conn conn-${connection}`} title="SSE connection to /events">
          {CONNECTION_LABEL[connection]}
        </span>
        <span className="count">{`${count} request${count === 1 ? "" : "s"}`}</span>
        <input
          type="search"
          placeholder="filter: host, path, method, status"
          autoComplete="off"
          spellCheck={false}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <label className="follow">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> follow
        </label>
        <button type="button" onClick={clear}>
          clear
        </button>
      </header>
      <main ref={scroller} onScroll={onScroll}>
        <table>
          <thead>
            <tr>
              <th className="c-time">time</th>
              <th className="c-method">method</th>
              <th className="c-host">host</th>
              <th className="c-path">path</th>
              <th className="c-status">status</th>
              <th className="c-dur">duration</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((e) => (
              <Row key={e.id} entry={e} open={open.has(e.id)} onToggle={toggle} />
            ))}
          </tbody>
        </table>
        {count === 0 ? (
          <p className="empty">
            No traffic yet. Send something through the proxy:
            <br />
            <code>curl -x http://127.0.0.1:8080 http://example.com</code>
          </p>
        ) : null}
      </main>
      <footer>For authorized testing only. This page shows raw traffic, including credentials.</footer>
    </>
  );
}

const Row = memo(function Row({
  entry,
  open,
  onToggle,
}: {
  entry: TrafficEntry;
  open: boolean;
  onToggle: (id: string) => void;
}) {
  return (
    <Fragment>
      <tr className={open ? "row open" : "row"} onClick={() => onToggle(entry.id)}>
        <td className="time" title={entry.ts}>
          {timeOf(entry.ts)}
        </td>
        <td>{entry.method}</td>
        <td>{entry.host}</td>
        <td title={entry.url}>
          {pathOf(entry.url)}
          {entry.replay_of ? <span className="replay">replay</span> : null}
        </td>
        <td className={statusClass(entry.status)}>{statusText(entry.status)}</td>
        <td className="dur">{durationText(entry.duration_ms)}</td>
      </tr>
      {open ? (
        <tr className="detail">
          <td colSpan={6}>
            <Detail entry={entry} />
          </td>
        </tr>
      ) : null}
    </Fragment>
  );
});
