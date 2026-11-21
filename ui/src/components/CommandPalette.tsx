// Ctrl+K / Cmd+K command palette (docs/ux.md). A small in-house component
// rather than cmdk: cmdk pulls ~25 runtime packages, including runtime
// <style> injection that spall's CSP blocks.

import { useEffect, useMemo, useRef, useState } from "react";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  /** Commands that take an argument switch the palette into input mode. */
  prompt?: string;
  run: (arg?: string) => void;
}

export function CommandPalette({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const [text, setText] = useState("");
  const [active, setActive] = useState(0);
  const [prompting, setPrompting] = useState<Command | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => input.current?.focus(), [prompting]);

  const matches = useMemo(() => {
    const q = text.trim().toLowerCase();
    return prompting ? [] : commands.filter((c) => c.label.toLowerCase().includes(q));
  }, [commands, text, prompting]);
  const current = Math.min(active, Math.max(matches.length - 1, 0));

  const choose = (c: Command | undefined) => {
    if (!c) return;
    if (c.prompt) {
      setPrompting(c);
      setText("");
      return;
    }
    onClose();
    c.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((current + 1) % Math.max(matches.length, 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((current - 1 + matches.length) % Math.max(matches.length, 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (prompting) {
        onClose();
        prompting.run(text.trim());
      } else {
        choose(matches[current]);
      }
    }
  };

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="palette" role="dialog" aria-label="Commands" onMouseDown={(e) => e.stopPropagation()}>
        <input
          ref={input}
          value={text}
          placeholder={prompting?.prompt ?? "Type a command"}
          aria-label={prompting?.prompt ?? "Command"}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => {
            setText(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        {prompting ? null : (
          <ul role="listbox" aria-label="Commands">
            {matches.map((c, i) => (
              <li
                key={c.id}
                role="option"
                aria-selected={i === current}
                className={i === current ? "on" : undefined}
                onMouseEnter={() => setActive(i)}
                onClick={() => choose(c)}
              >
                <span>{c.label}</span>
                {c.hint ? <kbd>{c.hint}</kbd> : null}
              </li>
            ))}
            {matches.length === 0 ? <li className="none">No matching command.</li> : null}
          </ul>
        )}
      </div>
    </div>
  );
}
