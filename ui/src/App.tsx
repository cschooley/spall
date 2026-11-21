import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { execute, paletteItems, type Ctx, type OutKind, type PaletteItem } from "./commands/registry.ts";
import { CommandPalette } from "./components/CommandPalette.tsx";
import { Console } from "./components/Console.tsx";
import { Icon } from "./components/Icon.tsx";
import { Inspector } from "./components/Inspector.tsx";
import { Launcher } from "./components/Launcher.tsx";
import { Rail } from "./components/Rail.tsx";
import { modKey, ShortcutSheet } from "./components/ShortcutSheet.tsx";
import { TrafficTable } from "./components/TrafficTable.tsx";
import { addLine, clearLog, emptyLog, markRead } from "./lib/consoleLog.ts";
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
const CONSOLE_KEY = "spall.consoleKey";
const PALETTE = paletteItems();

function useTheme() {
  const [theme, setThemeState] = useState<ThemeName>(() => loadTheme(browserStorage()));
  const setTheme = useCallback((name: ThemeName) => {
    setThemeState(name);
    applyTheme(name);
    saveTheme(browserStorage(), name);
  }, []);
  return [theme, setTheme] as const;
}

function loadConsoleKey(): string {
  try {
    const k = browserStorage()?.getItem(CONSOLE_KEY);
    return k && [...k].length === 1 ? k : "`";
  } catch {
    return "`";
  }
}

function ThemePicker({ theme, setTheme }: { theme: ThemeName; setTheme: (t: ThemeName) => void }) {
  const { brand, mode } = themeParts(theme);
  const name = (s: string) => s[0]!.toUpperCase() + s.slice(1);
  return (
    <>
      <div className="seg" role="group" aria-label="Metal">
        {(["rust", "blued"] as const).map((b: Brand) => (
          <button key={b} type="button" aria-pressed={b === brand} onClick={() => setTheme(themeName({ brand: b, mode }))}>
            {/* data-theme scopes that theme's tokens to the swatch: no color literals. */}
            <i className="sw" data-theme={b} aria-hidden />
            {name(b)}
          </button>
        ))}
      </div>
      <div className="seg" role="group" aria-label="Brightness" title="t toggles">
        {(["dark", "light"] as const).map((m: Mode) => (
          <button key={m} type="button" aria-pressed={m === mode} onClick={() => setTheme(themeName({ brand, mode: m }))}>
            <span className="glyph" aria-hidden>
              {m === "dark" ? "◐" : "○"}
            </span>
            {name(m)}
          </button>
        ))}
      </div>
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
  const [sheet, setSheet] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [consolePrefill, setConsolePrefill] = useState("");
  const [consoleKey, setConsoleKeyState] = useState(loadConsoleKey);
  const [log, setLog] = useState(() => addLine(emptyLog(), "spall console · type help", "sys", true));
  const [clearedAt, setClearedAt] = useState<string | null>(null);
  const filterInput = useRef<HTMLInputElement>(null);

  const out = useCallback(
    (text: string, kind: OutKind = "out") => setLog((l) => addLine(l, text, kind, consoleOpen)),
    [consoleOpen],
  );

  useEffect(() => {
    const id = setTimeout(() => setDebounced(filterText), 250);
    return () => clearTimeout(id);
  }, [filterText]);
  const query = useMemo(() => parseFilter(debounced), [debounced]);
  const history = useHistory(
    query,
    status.source === "log" ? "off" : status.source === "pending" ? "wait" : "fetch",
  );

  // System messages: note changes during render (React's pattern for state
  // derived from changing values), so they land in the console log once.
  const [seen, setSeen] = useState({ connection, feed: status.connected, historyError: history.error, ignored: "" });
  const ignored = query.ignored.join(" ");
  if (
    seen.connection !== connection ||
    seen.feed !== status.connected ||
    seen.historyError !== history.error ||
    seen.ignored !== ignored
  ) {
    const add = (text: string, kind: OutKind) => setLog((l) => addLine(l, text, kind, consoleOpen));
    if (seen.connection !== connection) {
      if (connection === "reconnecting") add("live stream lost; reconnecting", "warn");
      else if (connection === "down") add("live stream disconnected", "stop");
      else if (connection === "live" && seen.connection !== "connecting") add("live stream reconnected", "sys");
    }
    if (status.source === "lever" && seen.feed !== status.connected && status.connected !== null) {
      if (status.connected) add("taphttp feed connected", "sys");
      else add("taphttp feed disconnected", "warn");
    }
    if (seen.historyError !== history.error && history.error) add(history.error, "warn");
    if (seen.ignored !== ignored && ignored) add(`filter: ignored ${ignored} (attribution arrives with agents, M5)`, "warn");
    setSeen({ connection, feed: status.connected, historyError: history.error, ignored });
  }

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
    async (id: string): Promise<boolean> => {
      if (!ID_PATTERN.test(id)) return false;
      const loaded = rows.find((r) => r.id === id) ?? store.items.find((r) => r.id === id);
      if (loaded) {
        setSelected(loaded);
        return true;
      }
      if (status.source === "log") return false; // no history API to ask
      try {
        const res = await fetch(`/api/exchanges/${encodeURIComponent(id)}`);
        if (!res.ok) return false;
        setSelected((await res.json()) as TrafficEntry);
        return true;
      } catch {
        return false;
      }
    },
    [rows, store.items, status.source],
  );

  const focusFilter = useCallback(() => filterInput.current?.focus(), []);
  const toggleMode = useCallback(() => {
    const p = themeParts(theme);
    setTheme(themeName({ brand: p.brand, mode: p.mode === "dark" ? "light" : "dark" }));
  }, [theme, setTheme]);

  const openConsole = useCallback((prefill = "") => {
    setConsolePrefill(prefill);
    setConsoleOpen(true);
    setLog(markRead);
  }, []);
  const closeConsole = useCallback(() => setConsoleOpen(false), []);

  const ctx: Ctx = useMemo(
    () => ({
      out,
      setFilter: (q) => setFilterText(q),
      focusFilter,
      openById,
      selected: () => selected,
      curlText: () => (selected ? toCurl(selected) : null),
      copyCurl: async () => {
        if (!selected) return false;
        try {
          await navigator.clipboard.writeText(toCurl(selected));
          return true;
        } catch {
          return false;
        }
      },
      jumpToNewest: () => setFollow(true),
      clearTable: clearAll,
      setTheme,
      clearConsole: () => setLog(clearLog),
      openKeys: () => setSheet(true),
      setConsoleKey: (k) => {
        if ([...k].length !== 1) return false;
        setConsoleKeyState(k);
        try {
          browserStorage()?.setItem(CONSOLE_KEY, k);
        } catch {
          // storage blocked: the key lasts for this page only
        }
        return true;
      },
    }),
    [out, focusFilter, openById, selected, clearAll, setTheme],
  );

  const run = useCallback((line: string) => void execute(line, ctx), [ctx]);
  const choose = useCallback(
    (item: PaletteItem) => (item.prefill ? openConsole(item.prefill) : run(item.command)),
    [openConsole, run],
  );

  // Global keys. Ctrl/Cmd+K works everywhere; the rest are ignored while
  // typing in a field (the console handles its own toggle key).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
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
      if (e.key === consoleKey) {
        e.preventDefault();
        if (consoleOpen) closeConsole();
        else openConsole();
      } else if (e.key === "?") {
        e.preventDefault();
        setSheet((s) => !s);
      } else if (e.key === "Escape") {
        if (sheet) setSheet(false);
        else setSelected(null);
      } else if (sheet) {
        return;
      } else if (e.key === "/") {
        e.preventDefault();
        focusFilter();
      } else if (e.key === "j") move(1);
      else if (e.key === "k") move(-1);
      else if (e.key === "t") toggleMode();
      else if (e.key === "x") document.querySelector<HTMLButtonElement>(".inspector [data-action='hex']")?.click();
      // 1-5 will switch views once a second view ships (see Rail.tsx).
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, sheet, consoleKey, consoleOpen, openConsole, closeConsole, move, toggleMode, focusFilter]);

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
            <kbd>{`${modKey()} K`}</kbd> commands
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
          {status.proxy ? <span className="addr">{` ${status.proxy}`}</span> : null}
        </span>
        <span>{`${rows.length} exchange${rows.length === 1 ? "" : "s"}`}</span>
        {log.unread > 0 && !consoleOpen ? (
          <button type="button" className="con-unread" title={`New console messages (press ${consoleKey})`} onClick={() => openConsole()}>
            {`${consoleKey} ${log.unread}`}
          </button>
        ) : null}
        <span className="spacer" />
        <span className="keys">
          <kbd>?</kbd> keys <kbd>{consoleKey}</kbd> console <kbd>t</kbd> light/dark <kbd>1</kbd>–<kbd>5</kbd> views <kbd>/</kbd> filter{" "}
          <kbd>j</kbd>
          <kbd>k</kbd> move
        </span>
        <span className="legal">authorized testing only</span>
      </footer>

      {consoleOpen ? (
        <Console
          key={consolePrefill}
          log={log}
          toggleKey={consoleKey}
          prefill={consolePrefill}
          onRun={run}
          onClose={closeConsole}
          onHint={(t) => out(t, "sys")}
        />
      ) : null}
      {palette ? (
        <CommandPalette items={PALETTE} consoleKey={consoleKey} onChoose={choose} onClose={() => setPalette(false)} />
      ) : null}
      {sheet ? <ShortcutSheet consoleKey={consoleKey} onClose={() => setSheet(false)} /> : null}
    </div>
  );
}
