// spall M0 console. No framework, no build step.
//
// Every value shown here comes from captured traffic and is attacker
// controlled. Only ever put it into the DOM via textContent / text nodes,
// never innerHTML. The page CSP blocks inline script as a backstop.
"use strict";

const MAX_ROWS = 5000;
const MAX_BODY_CHARS = 256 * 1024;

const rowsEl = document.getElementById("rows");
const scroller = document.getElementById("scroller");
const connEl = document.getElementById("conn");
const countEl = document.getElementById("count");
const emptyEl = document.getElementById("empty");
const filterEl = document.getElementById("filter");
const followEl = document.getElementById("follow");

const seen = new Set();   // entry ids already rendered (SSE reconnects replay history)
const items = [];         // { entry, row, detail, haystack }

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined && text !== null) e.textContent = String(text);
  return e;
}

function pathOf(url) {
  if (typeof url !== "string") return "";
  if (url.startsWith("/")) return url;
  try {
    const u = new URL(url);
    return u.pathname + u.search;
  } catch (_) {
    return url;
  }
}

function timeOf(ts) {
  const d = new Date(ts);
  if (isNaN(d)) return String(ts);
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function statusClass(s) {
  if (s === null || s === undefined) return "st-err";
  const k = Math.floor(s / 100);
  return k >= 2 && k <= 5 ? `st-${k}` : "";
}

function headersPre(h) {
  const names = Object.keys(h || {}).sort();
  if (names.length === 0) return el("pre", "none", "(none)");
  return el("pre", null, names.map((n) => `${n}: ${h[n]}`).join("\n"));
}

function bodyPre(body, headers) {
  if (body === null || body === undefined || body === "") return el("pre", "none", "(empty)");
  let text = body;
  const ct = Object.entries(headers || {}).find(([k]) => k.toLowerCase() === "content-type");
  if ((ct && /json/i.test(ct[1])) || /^\s*[[{]/.test(text)) {
    try { text = JSON.stringify(JSON.parse(text), null, 2); } catch (_) { /* show as-is */ }
  }
  if (text.length > MAX_BODY_CHARS) {
    text = text.slice(0, MAX_BODY_CHARS) + `\n\n… truncated for display (${body.length} chars total)`;
  }
  return el("pre", null, text);
}

function buildDetail(entry) {
  const tr = el("tr", "detail");
  const td = el("td");
  td.colSpan = 6;

  const meta = el("div", "meta");
  meta.append(el("b", null, entry.method + " "), document.createTextNode(entry.url));
  meta.append(document.createElement("br"), document.createTextNode(`id ${entry.id}`));
  if (entry.replay_of) meta.append(document.createTextNode(` · replay of ${entry.replay_of}`));
  td.append(meta);

  const grid = el("div", "detail-grid");
  const req = el("section");
  req.append(el("h3", null, "request headers"), headersPre(entry.req_headers),
             el("h3", null, "request body"), bodyPre(entry.req_body, entry.req_headers));
  const res = el("section");
  res.append(el("h3", null, "response headers"), headersPre(entry.res_headers),
             el("h3", null, "response body"), bodyPre(entry.res_body, entry.res_headers));
  grid.append(req, res);
  td.append(grid);
  tr.append(td);
  return tr;
}

function matches(item, q) {
  return q === "" || item.haystack.includes(q);
}

function addEntry(entry) {
  if (!entry || typeof entry.id !== "string" || seen.has(entry.id)) return;
  seen.add(entry.id);

  const path = pathOf(entry.url);
  const status = entry.status === null || entry.status === undefined ? "ERR" : entry.status;
  const row = el("tr", "row");
  row.append(
    el("td", "time", timeOf(entry.ts)),
    el("td", null, entry.method),
    el("td", null, entry.host),
  );
  const pathTd = el("td", null, path);
  pathTd.title = entry.url;
  if (entry.replay_of) pathTd.append(el("span", "replay", "replay"));
  row.append(
    pathTd,
    el("td", statusClass(entry.status), status),
    el("td", "dur", entry.duration_ms === null || entry.duration_ms === undefined ? "" : `${entry.duration_ms} ms`),
  );

  const item = {
    entry, row, detail: null,
    haystack: `${entry.method} ${entry.host} ${path} ${status}`.toLowerCase(),
  };
  row.addEventListener("click", () => toggle(item));
  row.hidden = !matches(item, filterEl.value.trim().toLowerCase());

  items.push(item);
  rowsEl.append(row);
  while (items.length > MAX_ROWS) {
    const old = items.shift();
    old.row.remove();
    if (old.detail) old.detail.remove();
    seen.delete(old.entry.id);
  }
  updateCount();
}

function toggle(item) {
  if (item.detail) {
    item.detail.remove();
    item.detail = null;
    item.row.classList.remove("open");
  } else {
    item.detail = buildDetail(item.entry);
    item.row.after(item.detail);
    item.row.classList.add("open");
  }
}

function updateCount() {
  const n = items.length;
  countEl.textContent = `${n} request${n === 1 ? "" : "s"}`;
  emptyEl.hidden = n > 0;
}

function setConn(state, label) {
  connEl.className = `conn conn-${state}`;
  connEl.textContent = label;
}

function scrollToBottom() {
  if (followEl.checked) scroller.scrollTop = scroller.scrollHeight;
}

// Stop following when the user scrolls up; resume at the bottom.
let programmaticScroll = false;
scroller.addEventListener("scroll", () => {
  if (programmaticScroll) return;
  const atBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
  followEl.checked = atBottom;
});

filterEl.addEventListener("input", () => {
  const q = filterEl.value.trim().toLowerCase();
  for (const item of items) {
    const show = matches(item, q);
    item.row.hidden = !show;
    if (item.detail) item.detail.hidden = !show;
  }
});

document.getElementById("clear").addEventListener("click", () => {
  // Keep ids in `seen` so a reconnect does not re-add cleared history.
  for (const item of items) {
    item.row.remove();
    if (item.detail) item.detail.remove();
  }
  items.length = 0;
  updateCount();
});

let pending = false;
const source = new EventSource("/events");
source.onopen = () => setConn("live", "live");
source.onerror = () => {
  setConn(source.readyState === EventSource.CLOSED ? "down" : "connecting",
          source.readyState === EventSource.CLOSED ? "disconnected" : "reconnecting");
};
source.onmessage = (ev) => {
  let entry;
  try { entry = JSON.parse(ev.data); } catch (_) { return; }
  addEntry(entry);
  if (!pending) {
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      programmaticScroll = true;
      scrollToBottom();
      requestAnimationFrame(() => { programmaticScroll = false; });
    });
  }
};

updateCount();
