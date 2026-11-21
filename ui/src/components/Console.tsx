// The ` console (docs/ux.md): typed commands from the shared registry, with
// history, tab completion and scrollback. System messages land here too.
// Text only: every line is a JSX text node, never HTML.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { complete } from "../commands/registry.ts";
import type { ConsoleLog } from "../lib/consoleLog.ts";

export function Console({
  log,
  toggleKey,
  prefill,
  onRun,
  onClose,
  onHint,
}: {
  log: ConsoleLog;
  toggleKey: string;
  /** Text to start the input with (palette commands that need arguments). */
  prefill: string;
  onRun: (line: string) => void;
  onClose: () => void;
  /** Show completion options (as system output). */
  onHint: (text: string) => void;
}) {
  const [text, setText] = useState(prefill);
  const [history, setHistory] = useState<string[]>([]);
  const [pos, setPos] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = input.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  useLayoutEffect(() => {
    if (scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [log.lines.length]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape" || e.key === toggleKey) {
      e.preventDefault();
      onClose();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const line = text.trim();
      if (line) {
        setHistory((h) => (h.at(-1) === line ? h : h.concat(line)));
        setPos(history.at(-1) === line ? history.length : history.length + 1);
        onRun(line);
      }
      setText("");
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (pos > 0) {
        setPos(pos - 1);
        setText(history[pos - 1] ?? "");
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      const next = Math.min(history.length, pos + 1);
      setPos(next);
      setText(history[next] ?? "");
    } else if (e.key === "Tab") {
      e.preventDefault();
      const c = complete(text);
      if (c.line !== null) setText(c.line);
      else if (c.options.length) onHint(c.options.join("  "));
    }
  };

  return (
    <section className="console" aria-label="Console">
      <div className="con-log" role="log" aria-live="polite" ref={scroller}>
        {log.lines.map((l) => (
          <div key={l.id} className={`l-${l.kind}`}>
            {l.text}
          </div>
        ))}
      </div>
      <div className="con-in">
        <span className="con-prompt" aria-hidden>
          ]
        </span>
        <input
          ref={input}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          autoComplete="off"
          spellCheck={false}
          aria-label="Console command"
          placeholder="type help"
        />
        <span className="con-hint">{`${toggleKey} close · ↑↓ history · tab complete`}</span>
      </div>
    </section>
  );
}
