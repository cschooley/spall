import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { execute, paletteItems, type Ctx, type OutKind, type PaletteItem, type ReplayEdit } from "./commands/registry.ts";
import { CommandPalette } from "./components/CommandPalette.tsx";
import { Console } from "./components/Console.tsx";
import { Icon } from "./components/Icon.tsx";
import { Inspector, type Tab as InspectorTab } from "./components/Inspector.tsx";
import { Launcher } from "./components/Launcher.tsx";
import { Rail, VIEWS } from "./components/Rail.tsx";
import { modKey, ShortcutSheet } from "./components/ShortcutSheet.tsx";
import { TrafficTable } from "./components/TrafficTable.tsx";
import { addLine, clearLog, emptyLog, markRead } from "./lib/consoleLog.ts";
import { toCurl } from "./lib/curl.ts";
import { durationText, shortId, statusLine } from "./lib/format.ts";
import { apiJson, asEngagement, asSummaries, viewFromHash, type ViewId } from "./lib/engagement.ts";
import { EngagementView } from "./components/EngagementView.tsx";
import { mergeExchanges, tsKey } from "./lib/merge.ts";
import { applyHeaderEdits, draftOf, replayBodyFor, type Draft } from "./lib/edit.ts";
import { ARM_MS, armedFor, armLabel, postReplay, type Armed, type ReplayBody } from "./lib/replay.ts";
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
import { replaysOf } from "./lib/replays.ts";
import { useChain, useOriginal, useReplayHistory } from "./useReplays.ts";
import { useSourceStatus } from "./useStatus.ts";
import { useHosts } from "./useHosts.ts";
import { asListing, stateLookup } from "./lib/hosts.ts";
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

/** The host a replay goes to, for the armed label. */
function hostOf(e: TrafficEntry): string {
  try {
    return new URL(e.url).host || e.host;
  } catch {
    return e.host;
  }
}

export function App() {
  // Views (docs/ux.md): the location hash names one; keys 1-5 switch to
  // the built ones (Rail.tsx).
  const [view, setView] = useState<ViewId>(() => viewFromHash(window.location.hash));
  useEffect(() => {
    const onHash = () => setView(viewFromHash(window.location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const { store, connection, clear } = useTraffic();
  const [statusNonce, setStatusNonce] = useState(0);
  const status = useSourceStatus(5000, statusNonce);
  // Host states (#39): polled while the engagement exists (lever API).
  const hosts = useHosts(status.replay, statusNonce);
  const hostOfState = useMemo(() => stateLookup(hosts.listing), [hosts.listing]);
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
  // Replay (#18): one state for the button, ⏎ and the console.
  const [armed, setArmed] = useState<Armed | null>(null);
  const [replayBusy, setReplayBusy] = useState<string | null>(null);
  const [replayMsg, setReplayMsg] = useState<{ id: string; text: string } | null>(null);

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
    const merged = mergeExchanges(history.items, store.items, (e) => matchesQuery(e, query, (h) => hostOfState(h)?.state ?? null));
    return clearedAt ? merged.filter((e) => tsKey(e.ts) > clearedAt) : merged;
  }, [history.items, store.items, query, clearedAt, hostOfState]);

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

  /** An exchange by id: loaded rows first, then taphttp's history. */
  const findEntry = useCallback(
    async (id: string): Promise<TrafficEntry | null> => {
      if (!ID_PATTERN.test(id)) return null;
      const loaded = rows.find((r) => r.id === id) ?? store.items.find((r) => r.id === id);
      if (loaded) return loaded;
      if (status.source === "log") return null; // no history API to ask
      try {
        const res = await fetch(`/api/exchanges/${encodeURIComponent(id)}`);
        return res.ok ? ((await res.json()) as TrafficEntry) : null;
      } catch {
        return null;
      }
    },
    [rows, store.items, status.source],
  );

  const openById = useCallback(
    async (id: string): Promise<boolean> => {
      const found = await findEntry(id);
      if (found) setSelected(found);
      return found !== null;
    },
    [findEntry],
  );

  // Arming lapses after ARM_MS, and on selecting anything else.
  useEffect(() => {
    if (!armed) return;
    const id = setTimeout(() => setArmed(null), Math.max(0, armed.until - Date.now()));
    return () => clearTimeout(id);
  }, [armed]);
  const [armedOn, setArmedOn] = useState(selected?.id ?? null);
  if (armedOn !== (selected?.id ?? null)) {
    setArmedOn(selected?.id ?? null);
    setArmed(null);
  }

  const replay = useCallback(
    async (id: string, opts: { yes: boolean; edit?: ReplayEdit; draft?: Draft }) => {
      const short = shortId(id);
      const refuse = (text: string) => {
        out(`replay ${short}: ${text}`, "warn");
        setReplayMsg({ id, text });
      };
      if (!status.replay) return refuse("replay needs taphttp's lever API (log-file mode)");
      let known: TrafficEntry | null = selected?.id === id ? selected : (rows.find((r) => r.id === id) ?? null);

      // Edits (#19): the editor's draft, or the console's flags applied to
      // the captured request. Only what changed is sent.
      let request: ReplayBody = { id };
      if (opts.draft || opts.edit) {
        known ??= await findEntry(id);
        if (!known) return refuse(`no exchange ${id}`);
        let draft = opts.draft;
        if (!draft) {
          const e = opts.edit!;
          draft = draftOf(known);
          if (e.method) draft.method = e.method;
          draft.headers = applyHeaderEdits(draft.headers, e.sets, e.drops);
          if (e.body !== undefined) {
            if (draft.body === null) return refuse("the captured body is binary; --body can't replace it");
            draft.body = e.body;
          }
        }
        const built = replayBodyFor(known, draft);
        if (!built.ok) return refuse(built.error);
        request = built.body;
        if (built.contentLength !== null) out(`replay ${short}: Content-Length set to ${built.contentLength}`, "sys");
      }

      const arm = armedFor(armed, id, Date.now());
      setArmed(null);
      setReplayMsg(null);
      setReplayBusy(id);
      let result = await postReplay({ ...request, confirm: arm?.token });
      if (result.kind === "confirm" && opts.yes) result = await postReplay({ ...request, confirm: result.token });
      setReplayBusy(null);
      switch (result.kind) {
        case "sent": {
          const e = result.entry;
          out(`replay ${short} → ${statusLine(e.status)}${e.duration_ms !== null ? ` in ${durationText(e.duration_ms)}` : ""}, recorded as ${shortId(e.id)}`, "sys");
          setSelected(e);
          break;
        }
        case "failed":
          out(`replay ${short} failed: ${result.entry.error ?? "no response"}; recorded as ${shortId(result.entry.id)}`, "warn");
          setSelected(result.entry);
          break;
        case "confirm": {
          const label = armLabel(request.method ?? known?.method ?? "it", known ? hostOf(known) : null);
          setArmed({ id, token: result.token, label, until: Date.now() + ARM_MS });
          out(`replay ${short}: ${result.reason}. ${label}, or replay ${id} --yes`, "sys");
          break;
        }
        default:
          out(`replay ${short}: ${result.reason}`, "warn");
          setReplayMsg({ id, text: result.reason });
      }
    },
    [status.replay, armed, out, selected, rows, findEntry],
  );

  // The open exchange's replays and chain (#21).
  const replayHistory = useReplayHistory(
    selected?.id ?? null,
    status.source === "log" ? "off" : status.source === "pending" ? "wait" : "fetch",
  );
  const replayList = useMemo(
    () => ({
      items: selected ? replaysOf(selected.id, [replayHistory.items, store.items, history.items]) : [],
      complete: replayHistory.complete,
      error: replayHistory.error,
    }),
    [selected, replayHistory, store.items, history.items],
  );
  const chain = useChain(selected, findEntry);
  const original = useOriginal(selected, findEntry);
  // Opening an exchange on a given tab (Compare from the Replays list).
  const [openTab, setOpenTab] = useState<{ id: string; tab: InspectorTab } | null>(null);

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
      replay,
      scope: () => (status.replay ? { entries: status.scope, all: status.scopeAll } : null),
      changeScope: async (change) => {
        try {
          const res = await fetch("/api/scope", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(change),
          });
          const body = (await res.json()) as { scope?: unknown; all?: unknown; changed?: unknown; error?: unknown };
          if (!res.ok) return typeof body.error === "string" ? body.error : `HTTP ${res.status}`;
          setStatusNonce((n) => n + 1);
          return {
            scope: Array.isArray(body.scope) ? body.scope.filter((s): s is string => typeof s === "string") : [],
            all: body.all === true,
            changed: body.changed === true,
          };
        } catch {
          return "spall unreachable";
        }
      },
      engagement: async () => {
        const body = await apiJson("/api/engagement");
        return typeof body === "string" ? body : (asEngagement(body) ?? "unexpected answer");
      },
      listEngagements: async () => {
        const body = await apiJson("/api/engagements");
        return typeof body === "string" ? body : (asSummaries(body) ?? "unexpected answer");
      },
      newEngagement: async (name) => {
        const body = await apiJson("/api/engagements", { method: "POST", body: { name } });
        if (typeof body === "string") return body;
        setStatusNonce((n) => n + 1);
        return asEngagement(body) ?? "unexpected answer";
      },
      useEngagement: async (which) => {
        const body = await apiJson("/api/engagements/active", { method: "POST", body: { id: which } });
        if (typeof body === "string") return body;
        setStatusNonce((n) => n + 1);
        return asEngagement(body) ?? "unexpected answer";
      },
      setEngagement: async (field, value) => {
        const body = await apiJson("/api/engagement", { method: "PATCH", body: { [field]: value } });
        if (typeof body === "string") return body;
        setStatusNonce((n) => n + 1);
        const o = body as { engagement?: unknown; changed?: unknown };
        const engagement = asEngagement(o.engagement);
        return engagement ? { engagement, changed: o.changed === true } : "unexpected answer";
      },
      hosts: async () => {
        const body = await apiJson("/api/hosts");
        return typeof body === "string" ? body : (asListing(body) ?? "unexpected answer");
      },
      openScopePaste: () => {
        window.location.hash = "engagement/paste";
      },
      setRules: async (patch) => {
        const body = await apiJson("/api/engagement", { method: "PATCH", body: { rules: patch } });
        if (typeof body === "string") return body;
        setStatusNonce((n) => n + 1);
        const o = body as { engagement?: unknown; changed?: unknown };
        // PATCH returns the engagement without testing_now: fetch it fresh.
        const fresh = await apiJson("/api/engagement");
        const engagement = asEngagement(typeof fresh === "string" ? o.engagement : fresh);
        return engagement ? { engagement, changed: o.changed === true } : "unexpected answer";
      },
      checkScope: async (target) => {
        try {
          const res = await fetch(`/api/scope/check?target=${encodeURIComponent(target)}`);
          const body = (await res.json()) as { host?: unknown; state?: unknown; by?: unknown; error?: unknown };
          if (!res.ok) return typeof body.error === "string" ? body.error : `HTTP ${res.status}`;
          const state = body.state === "in" || body.state === "out" ? body.state : "unknown";
          return { host: typeof body.host === "string" ? body.host : target, state, by: typeof body.by === "string" ? body.by : null };
        } catch {
          return "spall unreachable";
        }
      },
      audit: async (last) => {
        try {
          const res = await fetch(`/api/audit?last=${last}`);
          const body: unknown = await res.json();
          if (!res.ok) {
            const msg = (body as { error?: unknown } | null)?.error;
            return typeof msg === "string" ? msg : `HTTP ${res.status}`;
          }
          return Array.isArray(body) ? (body as Record<string, unknown>[]) : "unexpected answer";
        } catch {
          return "spall unreachable";
        }
      },
    }),
    [out, focusFilter, openById, selected, clearAll, setTheme, replay, status.replay, status.scope, status.scopeAll],
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
      const goto = VIEWS.find((v) => v.built && v.key === e.key && !sheet);
      if (goto) {
        e.preventDefault();
        window.location.hash = goto.id;
        return;
      }
      if (e.key !== "Enter" && !["Shift", "Control", "Alt", "Meta"].includes(e.key)) setArmed(null);
      if (e.key === "Enter") {
        // A focused button or tab takes Enter itself (the Replay button's
        // own click is the press); otherwise ⏎ replays the open exchange.
        if ((e.target as HTMLElement | null)?.closest("button, a, [role='tab']")) return;
        if (selected && !sheet) {
          e.preventDefault();
          void replay(selected.id, { yes: false });
        }
        return;
      }
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
      } else if (e.key === "t") toggleMode();
      else if (view !== "traffic") {
        // The rest are Traffic's keys.
        return;
      } else if (e.key === "/") {
        e.preventDefault();
        focusFilter();
      } else if (e.key === "j") move(1);
      else if (e.key === "k") move(-1);
      else if (e.key === "x") document.querySelector<HTMLButtonElement>(".inspector [data-action='hex']")?.click();
      else if (e.key === "e") document.querySelector<HTMLButtonElement>(".inspector [data-action='edit']")?.click();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, sheet, consoleKey, consoleOpen, openConsole, closeConsole, move, toggleMode, focusFilter, selected, replay, view]);

  // Any click outside the Replay button disarms it.
  useEffect(() => {
    if (!armed) return;
    const onDown = (e: MouseEvent) => {
      if (!(e.target as HTMLElement | null)?.closest("[data-action='replay']")) setArmed(null);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [armed]);

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

      <Rail current={view} />

      {view === "engagement" ? (
        <main className="eng-main">
          <EngagementView available={status.replay} hosts={hosts.listing} onChanged={() => setStatusNonce((n) => n + 1)} />
        </main>
      ) : (
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
            hostState={hosts.listing ? hostOfState : null}
            selectedId={selected?.id ?? null}
            onSelect={select}
            follow={follow}
            setFollow={setFollow}
            onNearTop={history.loadOlder}
            appliedFilter={debounced}
            empty={isEmptyQuery(query) ? "No traffic. Point a client at 127.0.0.1:8080." : "No exchanges match this filter."}
          />
          {selected ? (
            <Inspector
              key={selected.id}
              entry={selected}
              onClose={() => setSelected(null)}
              onOpen={(id, tab) => {
                setOpenTab(tab ? { id, tab } : null);
                void openById(id).then((ok) => ok || out(`open: no exchange ${id}`, "warn"));
              }}
              original={original}
              initialTab={openTab?.id === selected.id ? openTab.tab : "request"}
              replays={replayList}
              chain={chain}
              replay={{
                available: status.replay,
                busy: replayBusy === selected.id,
                // The expiry timer clears `armed`, so no clock read here.
                armedLabel: armed?.id === selected.id ? armed.label : null,
                message: replayMsg?.id === selected.id ? replayMsg.text : null,
                onReplay: (draft) => void replay(selected.id, draft ? { yes: false, draft } : { yes: false }),
                onDraftChange: () => setArmed(null),
              }}
            />
          ) : null}
        </main>
      )}

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
        {status.engagement ? (
          <span className="engagement-name" title={`Engagement ${status.engagement.id} (engagement in the console)`}>
            {status.engagement.name}
          </span>
        ) : null}
        {status.testing && !status.testing.allowed ? (
          <span className="testing-closed" title={status.testing.reason ?? "The rules of engagement don't allow testing now (roe in the console)."}>
            ● not testing now
          </span>
        ) : null}
        {status.scopeAll ? (
          <span className="scope-all" title="Replays may go to any host you captured, third parties included. scope all off in the console.">
            ● scope: all hosts
          </span>
        ) : null}
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
