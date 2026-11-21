// Ctrl+K / Cmd+K palette: find a command you don't remember. Entries come
// from the shared registry (commands/registry.ts); ones that need arguments
// open the console prefilled. In-house rather than cmdk: cmdk pulls ~25
// runtime packages, including runtime <style> injection the CSP blocks.
import { useEffect, useMemo, useRef, useState } from "react";
import type { PaletteItem } from "../commands/registry.ts";

/** Fuzzy match: the query's characters appear in order. */
export function fuzzy(query: string, text: string): boolean {
  const q = query.toLowerCase();
  let i = 0;
  for (const ch of text.toLowerCase()) if (ch === q[i]) i++;
  return i === q.length;
}

export function CommandPalette({
  items,
  consoleKey,
  onChoose,
  onClose,
}: {
  items: PaletteItem[];
  consoleKey: string;
  onChoose: (item: PaletteItem) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  const matches = useMemo(() => {
    const q = text.trim();
    return items.filter((it) => !q || fuzzy(q, `${it.label} ${it.hint} ${it.group}`));
  }, [items, text]);
  const current = Math.min(active, Math.max(matches.length - 1, 0));

  const choose = (it: PaletteItem | undefined) => {
    if (!it) return;
    onClose();
    onChoose(it);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive(Math.min(matches.length - 1, current + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive(Math.max(0, current - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(matches[current]);
    }
  };

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div className="pal" role="dialog" aria-modal="true" aria-label="Command palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          ref={input}
          value={text}
          placeholder="Type a command…"
          aria-label="Command"
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => {
            setText(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <ul role="listbox" aria-label="Commands">
          {matches.map((it, i) => (
            <li
              key={it.command}
              role="option"
              aria-selected={i === current}
              className={i === current ? "on" : undefined}
              onMouseEnter={() => setActive(i)}
              onClick={() => choose(it)}
            >
              <span className="grp">{it.group}</span>
              <span>{it.label}</span>
              <span className="hint">{it.hint}</span>
            </li>
          ))}
          {matches.length === 0 ? <li className="none">{`No match. The console takes any command: press ${consoleKey}`}</li> : null}
        </ul>
        <div className="pal-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>⏎</kbd> run
          </span>
          <span>
            <kbd>Esc</kbd> close
          </span>
        </div>
      </div>
    </div>
  );
}
