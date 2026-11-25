// Desktop alerts (#86): off until you switch them on (the browser asks
// once). Kept per browser; the alerts themselves are content-free.
import { useState } from "react";

const KEY = "spall.alerts";

export function alertsWanted(): boolean {
  try {
    return localStorage.getItem(KEY) === "on" && typeof Notification !== "undefined" && Notification.permission === "granted";
  } catch {
    return false;
  }
}

export function AlertsButton() {
  const supported = typeof Notification !== "undefined";
  const [on, setOn] = useState(alertsWanted);
  const [blocked, setBlocked] = useState(supported && Notification.permission === "denied");
  const save = (v: boolean) => {
    try {
      localStorage.setItem(KEY, v ? "on" : "off");
    } catch {
      // private window: on for this page only
    }
    setOn(v);
  };
  const toggle = async () => {
    if (on) return save(false);
    const p = Notification.permission === "granted" ? "granted" : await Notification.requestPermission();
    setBlocked(p === "denied");
    save(p === "granted");
  };
  const title = !supported
    ? "This browser has no desktop notifications; alerts still show in the console"
    : blocked
      ? "The browser blocks spall's notifications: allow them in its site settings"
      : on
        ? "Desktop alerts are on: tasks waiting or finished, OOB callbacks, quad. Click to turn off"
        : "Desktop alerts are off. Click to turn on (no hosts or findings in them)";
  return (
    <button type="button" className={on ? "alerts-toggle on" : "alerts-toggle"} aria-pressed={on} aria-label="Desktop alerts" title={title} disabled={!supported} onClick={() => void toggle()}>
      {on ? "alerts on" : "alerts off"}
    </button>
  );
}
