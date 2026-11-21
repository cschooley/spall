import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CommandPalette, type Command } from "./components/CommandPalette.tsx";
import { Icon } from "./components/Icon.tsx";
import { Inspector } from "./components/Inspector.tsx";
import { Launcher } from "./components/Launcher.tsx";
import { Rail } from "./components/Rail.tsx";
import { TrafficTable } from "./components/TrafficTable.tsx";
import { toCurl } from "./lib/curl.ts";
import { mergeExchanges, tsKey } from "./lib/merge.ts";
import { isEmptyQuery, matchesQuery, parseFilter } from "./lib/query.ts";
import { litSegments, requestRate } from "./lib/rate.ts";
import {
  applyTheme,
  browserStorage,
  loadTheme,
  saveTheme,
  themeName,
  themeParts,
  type Brand,
  type Mode,
  type ThemeName,
} from "./lib/theme.ts";
import type { TrafficEntry } from "./types.ts";
import { useHistory } from "./useHistory.ts";
import { useSourceStatus } from "./useStatus.ts";
import { useTraffic, type Connection } from "./useTraffic.ts";

const CONNECTION_LABEL: Record<Connection, string> = {
  connecting: "connecting",
  live: "live",
  reconnecting: "reconnecting",
  down: "disconnected",
};

const ID_PATTERN = /^[A-Za-z0-9_-]{1,128}$/;

function useTheme() {
  const [theme, setThemeState] = useState<ThemeName>(() => loadTheme(browserStorage()));
  const setTheme = useCallback((name: ThemeName) => {
    setThemeState(name);
    applyTheme(name);
    saveTheme(browserStorage(), name);
  }, []);
  return [theme, setTheme] as const;
}

function ThemePicker({ theme, setTheme }: { theme: ThemeName; setTheme: (t: ThemeName) => void }) {
  const { brand, mode } = themeParts(theme);
  const seg = <T extends string>(label: string, value: T, options: readonly T[], pick: (v: T) => void) => (
    <div className="seg" role="group" aria-label={label}>
      {options.map((o) => (
        <button key={o} type="button" aria-pressed={o === value} onClick={() => pick(o)}>
          {o[0]!.toUpperCase() + o.slice(1)}
        </button>
      ))}
    </div>
  );
  return (
    <>
      {seg("Theme", brand, ["rust", "blued"] as const, (b: Brand) => setTheme(themeName({ brand: b, mode })))}
      {seg("Brightness", mode, ["dark", "light"] as const, (m: Mode) => setTheme(themeName({ brand, mode: m })))}
    </>
  );
}

function RateMeter({ live }: { live: TrafficEntry[] }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  // Capture times of recent traffic (not arrival in the browser, so the
  // history replayed on connect doesn't read as a burst).
  const recent: number[] = [];
  for (let i = live.length - 1; i >= 0; i--) {
    const t = Date.parse(live[i]!.ts);
    if (Number.isNaN(t) || t < now - 10_000) break;
    recent.push(t);
  }
  const rate = requestRate(recent, now);
  const lit = litSegments(rate);
  return (
    <span className="meter" title="requests per second, last 10 s">
      <span className="segs" aria-hidden>
        {Array.from({ length: 10 }, (_, i) => (
          <i key={i} className={i < lit ? "on" : undefined} />
        ))}
      </span>
      <b>{rate.toFixed(1)}</b> req/s
    </span>
  );
}

export function App() {
  const { store, connection, clear } = useTraffic();
  const status = useSourceStatus();
  const [theme, setTheme] = useTheme();
  const [filterText, setFilterText] = useState("");
  const [debounced, setDebounced] = useState("");
  const [follow, setFollow] = useState(true);
  const [selected, setSelected] = useState<TrafficEntry | null>(null);
  const [palette, setPalette] = useState(false);
  const [clearedAt, setClearedAt] = useState<string | null>(null);
  const filterInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const id = setTimeout(() => setDebounced(filterText), 250);
    return () => clearTimeout(id);
  }, [filterText]);
  const query = useMemo(() => parseFilter(debounced), [debounced]);
  const history = useHistory(
    query,
    status.source === "log" ? "off" : status.source === "pending" ? "wait" : "fetch",
  );

  const rows = useMemo(() => {
    const merged = mergeExchanges(history.items, store.items, (e) => matchesQuery(e, query));
    return clearedAt ? merged.filter((e) => tsKey(e.ts) > clearedAt) : merged;
  }, [history.items, store.items, query, clearedAt]);

  const select = useCallback((e: TrafficEntry | null) => setSelected(e), []);
  const clearAll = useCallback(() => {
    const newest = rows.at(-1);
    setClearedAt(newest ? tsKey(newest.ts) : tsKey(new Date().toISOString()));
    clear();
    setSelected(null);
  }, [rows, clear]);

  const move = useCallback(
    (delta: number) => {
      if (rows.length === 0) return;
      const i = selected ? rows.findIndex((r) => r.id === selected.id) : -1;
      const next = i < 0 ? (delta > 0 ? 0 : rows.length - 1) : Math.min(rows.length - 1, Math.max(0, i + delta));
      setSelected(rows[next]!);
      setFollow(false);
    },
    [rows, selected],
  );

  const openById = useCallback(
    async (id: string) => {
      if (!ID_PATTERN.test(id)) return;
      const loaded = rows.find((r) => r.id === id) ?? store.items.find((r) => r.id === id);
      if (loaded) {
        setSelected(loaded);
        return;
      }
      if (status.source === "log") return; // no history API to ask
      try {
        const res = await fetch(`/api/exchanges/${encodeURIComponent(id)}`);
        if (res.ok) setSelected((await res.json()) as TrafficEntry);
      } catch {
        // not found or no history API: nothing to open
      }
    },
    [rows, store.items, status.source],
  );

  const focusFilter = useCallback(() => filterInput.current?.focus(), []);

  const toggleMode = useCallback(() => {
    const p = themeParts(theme);
    setTheme(themeName({ brand: p.brand, mode: p.mode === "dark" ? "light" : "dark" }));
  }, [theme, setTheme]);

  const commands: Command[] = useMemo(
    () => [
      { id: "filter", label: "Filter traffic", hint: "/", run: focusFilter },
      { id: "clear-filter", label: "Clear filter", run: () => setFilterText("") },
      { id: "open", label: "Open exchange by id…", prompt: "Exchange id", run: (id) => void openById(id ?? "") },
      ...(selected
        ? [
            {
              id: "curl",
              label: "Copy as curl",
              run: () => void navigator.clipboard.writeText(toCurl(selected)).catch(() => undefined),
            },
          ]
        : []),
      { id: "newest", label: "Jump to newest", run: () => setFollow(true) },
      { id: "clear", label: "Clear table", run: clearAll },
      { id: "mode", label: "Toggle light / dark", hint: "t", run: toggleMode },
      {
        id: "brand",
        label: "Switch theme Rust / Blued",
        run: () => {
          const p = themeParts(theme);
          setTheme(themeName({ brand: p.brand === "rust" ? "blued" : "rust", mode: p.mode }));
        },
      },
    ],
    [selected, openById, clearAll, toggleMode, theme, setTheme, focusFilter],
  );

  // Global keys, ignored while typing in a field (except Ctrl/Cmd+K).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
        return;
      }
      const typing = (e.target as HTMLElement | null)?.closest("input, textarea, select");
      if (typing) {
        if (e.key === "Escape") (e.target as HTMLElement).blur();
        return;
      }
      if (palette || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "/") {
        e.preventDefault();
        focusFilter();
      } else if (e.key === "j") move(1);
      else if (e.key === "k") move(-1);
      else if (e.key === "Escape") setSelected(null);
      else if (e.key === "t") toggleMode();
      // 1-5 will switch views once a second view ships (see Rail.tsx).
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, move, toggleMode, focusFilter]);

  const historyNote = history.unavailable
    ? "live only (log-file mode: no history API)"
    : history.error
      ? history.error
      : history.loading
        ? "loading history…"
        : history.hasMore
          ? "scroll up for older"
          : "";

  return (
    <div className="app">
      <header className="bar">
        <span className="brand">
          <Icon name="icons/spall" size={22} />
          spall
        </span>
        <span className={`conn conn-${connection}`} title="SSE connection to /events">
          {CONNECTION_LABEL[connection]}
        </span>
        <RateMeter live={store.items} />
        <span className="spacer" />
        <ThemePicker theme={theme} setTheme={setTheme} />
        <Launcher taphttpConnected={status.source === "lever" ? status.connected : null} />
      </header>

      <Rail current="traffic" />

      <main className={selected ? "with-inspector" : undefined}>
        <div className="toolbar">
          <input
            ref={filterInput}
            type="search"
            className="filter"
            aria-label="Filter"
            placeholder="filter   host:example.com  method:POST  status:4xx  text"
            autoComplete="off"
            spellCheck={false}
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
          />
          {query.ignored.length ? (
            <span className="warn" title="by: arrives with agent attribution (M5)">
              {`ignored: ${query.ignored.join(" ")}`}
            </span>
          ) : null}
          <label className="follow">
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> follow
          </label>
          <button type="button" onClick={clearAll}>
            clear
          </button>
          <span className="note">{historyNote}</span>
          <span className="spacer" />
          <button type="button" className="hint" onClick={() => setPalette(true)}>
            <kbd>Ctrl K</kbd> commands
          </button>
        </div>
        <TrafficTable
          rows={rows}
          selectedId={selected?.id ?? null}
          onSelect={select}
          follow={follow}
          setFollow={setFollow}
          onNearTop={history.loadOlder}
          appliedFilter={debounced}
          empty={isEmptyQuery(query) ? "No traffic. Point a client at 127.0.0.1:8080." : "No exchanges match this filter."}
        />
        {selected ? <Inspector key={selected.id} entry={selected} onClose={() => setSelected(null)} /> : null}
      </main>

      <footer className="statusbar">
        <span>
          {"taphttp "}
          {status.source === "lever" ? (
            <span className={status.connected ? "ok" : "bad"}>{status.connected ? "● connected" : "● disconnected"}</span>
          ) : status.source === "log" ? (
            <span>● log file</span>
          ) : status.source === "pending" ? (
            <span>…</span>
          ) : (
            <span className="bad">● unknown</span>
          )}
        </span>
        <span>{`${rows.length} exchange${rows.length === 1 ? "" : "s"}`}</span>
        <span className="spacer" />
        <span className="keys">
          <kbd>/</kbd> filter <kbd>j</kbd>
          <kbd>k</kbd> move <kbd>Esc</kbd> close <kbd>t</kbd> light/dark
        </span>
        <span className="legal">authorized testing only</span>
      </footer>

      {palette ? <CommandPalette commands={commands} onClose={() => setPalette(false)} /> : null}
    </div>
  );
}
