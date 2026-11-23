// The exchange list: virtualized (thousands of rows stay fast), follows the
// tail, and asks for older history when scrolled to the top.

import { AgentMark } from "./Attribution.tsx";
import { memo, useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { durationText, methodClass, pathOf, statusClass, statusText, timeOf } from "../lib/format.ts";
import type { TrafficEntry } from "../types.ts";
import type { HostRow } from "../lib/hosts.ts";

export const ROW_HEIGHT = 26; // --vg-row-dense

export function TrafficTable({
  rows,
  selectedId,
  onSelect,
  follow,
  setFollow,
  onNearTop,
  empty,
  appliedFilter,
  hostState,
}: {
  rows: TrafficEntry[];
  /** Each host's scope state (#39); null without an engagement. */
  hostState: ((host: string) => HostRow | null) | null;
  selectedId: string | null;
  onSelect: (e: TrafficEntry) => void;
  follow: boolean;
  setFollow: (f: boolean) => void;
  onNearTop: () => void;
  empty: ReactNode;
  /** The filter text the rows reflect (after debounce); for tests and a11y. */
  appliedFilter: string;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const programmatic = useRef(false);
  // TanStack Virtual returns fresh functions each render, so the React
  // Compiler can't auto-memoize this component. spall doesn't use the
  // compiler; the rule's warning is informational here.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
    getItemKey: (i) => rows[i]!.id,
  });

  // Keep the view anchored when older history is prepended above it.
  const firstId = useRef<string | null>(null);
  useLayoutEffect(() => {
    const el = scroller.current;
    const prev = firstId.current;
    firstId.current = rows[0]?.id ?? null;
    if (!el || !prev || follow) return;
    const shift = rows.findIndex((r) => r.id === prev);
    if (shift > 0) el.scrollTop += shift * ROW_HEIGHT;
  }, [rows, follow]);

  // Follow the tail.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!follow || !el || rows.length === 0) return;
    programmatic.current = true;
    el.scrollTop = el.scrollHeight;
    requestAnimationFrame(() => {
      programmatic.current = false;
    });
  }, [rows.length, follow]);

  // Keep the selected row visible (j/k).
  useEffect(() => {
    if (!selectedId) return;
    const i = rows.findIndex((r) => r.id === selectedId);
    if (i >= 0) virtualizer.scrollToIndex(i, { align: "auto" });
  }, [selectedId, rows, virtualizer]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el || programmatic.current) return;
    setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
    if (el.scrollTop < ROW_HEIGHT * 8) onNearTop();
  };

  const items = virtualizer.getVirtualItems();
  const top = items[0]?.start ?? 0;
  const bottom = virtualizer.getTotalSize() - (items.at(-1)?.end ?? 0);

  return (
    <div className="tablewrap" ref={scroller} onScroll={onScroll} data-filter={appliedFilter}>
      <table>
        {/* Widths live on <col>: it inherits the table's mono font, so ch
            measures data digits (the header font is smaller). */}
        <colgroup>
          <col className="c-time" />
          <col className="c-method" />
          <col className="c-host" />
          <col className="c-path" />
          <col className="c-status" />
          <col className="c-dur" />
        </colgroup>
        <thead>
          <tr>
            <th className="c-time">time</th>
            <th className="c-method">method</th>
            <th className="c-host">host</th>
            <th className="c-path">path</th>
            <th className="c-status">status</th>
            <th className="c-dur">ms</th>
          </tr>
        </thead>
        <tbody>
          {top > 0 ? <tr aria-hidden style={{ height: top }} /> : null}
          {items.map((v) => {
            const e = rows[v.index]!;
            return <Row key={e.id} entry={e} selected={e.id === selectedId} onSelect={onSelect} scope={hostState ? (hostState(e.host) ?? undefined) : null} />;
          })}
          {bottom > 0 ? <tr aria-hidden style={{ height: bottom }} /> : null}
        </tbody>
      </table>
      {rows.length === 0 ? <p className="empty">{empty}</p> : null}
    </div>
  );
}

const SCOPE_TITLE = { in: "in scope", out: "excluded from scope", unknown: "not in scope (undecided)" } as const;

const Row = memo(function Row({
  entry,
  selected,
  onSelect,
  scope,
}: {
  entry: TrafficEntry;
  selected: boolean;
  onSelect: (e: TrafficEntry) => void;
  /** null: no engagement (no marker); undefined: not seen by spall yet. */
  scope: HostRow | null | undefined;
}) {
  const state = scope === null ? null : (scope?.state ?? "unknown");
  return (
    <tr className={selected ? "row sel" : "row"} aria-selected={selected} onClick={() => onSelect(entry)}>
      <td className="time" title={entry.ts}>
        {timeOf(entry.ts)}
      </td>
      <td className={`m ${methodClass(entry.method)}`}>{entry.method}</td>
      <td className="host">
        {state ? (
          <span className={`sc sc-${state}`} data-scope={state} title={`${SCOPE_TITLE[state]}${scope?.by ? ` (${scope.by})` : ""}`} aria-label={SCOPE_TITLE[state]}>
            ●
          </span>
        ) : null}
        {entry.host}
      </td>
      <td title={entry.url}>
        {pathOf(entry.url)}
        {entry.replay_of ? <span className="replay">replay</span> : null}
        <AgentMark meta={entry.meta} />
      </td>
      <td className={statusClass(entry.status)} title={entry.error ?? undefined}>
        {statusText(entry.status)}
      </td>
      <td className="dur">{entry.duration_ms === null ? "" : durationText(entry.duration_ms).replace(" ms", "")}</td>
    </tr>
  );
});
