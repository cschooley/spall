// The ? sheet. Lists only keys that work today; keys for later features
// (task the agent) appear when those ship.
const isMac = () => /Mac|iPhone|iPad/.test(navigator.platform || "");

export function modKey(): string {
  return isMac() ? "⌘" : "Ctrl";
}

export function ShortcutSheet({ consoleKey, onClose }: { consoleKey: string; onClose: () => void }) {
  const global: [string[], string][] = [
    [[`${modKey()} K`], "Command palette"],
    [[consoleKey], "Console"],
    [["?"], "This sheet"],
    [["1", "–", "5"], "Switch view"],
    [["t"], "Light / dark"],
    [["Shift", "Esc"], "Stop every agent task (e-stop)"],
  ];
  const traffic: [string[], string][] = [
    [["/"], "Filter"],
    [["j", "k"], "Next / previous row"],
    [["⏎"], "Replay (unsafe methods: press twice)"],
    [["e"], "Edit & replay"],
    [["f"], "Flag as candidate"],
    [["Esc"], "Close the inspector"],
    [["x"], "Hex view (binary body)"],
  ];
  const rows = (list: [string[], string][]) =>
    list.map(([keys, what]) => (
      <div key={what} className="row">
        <dt>
          {keys.map((k, i) => (k === "–" ? <span key={i}> – </span> : <kbd key={i}>{k}</kbd>))}
        </dt>
        <dd>{what}</dd>
      </div>
    ));
  return (
    <div className="overlay" onMouseDown={onClose}>
      <div className="sheet" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" onMouseDown={(e) => e.stopPropagation()}>
        <h2>Keyboard shortcuts</h2>
        <div className="cols2">
          <dl aria-label="Everywhere">{rows(global)}</dl>
          <dl aria-label="Traffic">{rows(traffic)}</dl>
        </div>
        <p className="note">
          Press <kbd>?</kbd> anytime. <kbd>Esc</kbd> closes.
        </p>
      </div>
    </div>
  );
}
