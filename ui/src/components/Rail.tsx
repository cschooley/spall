// The view rail (docs/ux.md): all five views, always in this order, keys
// 1-5. Views that aren't built yet are shown dimmed and inert; each lights
// up when it ships. A finished release has no dimmed icons.
import { Icon, type IconName } from "./Icon.tsx";

export interface View {
  id: string;
  label: string;
  key: string;
  icon: IconName;
  built: boolean;
}

export const VIEWS: readonly View[] = [
  { id: "engagement", label: "Engagement", key: "1", icon: "icons/views/engagement", built: false },
  { id: "traffic", label: "Traffic", key: "2", icon: "icons/views/traffic", built: true },
  { id: "surface", label: "Surface", key: "3", icon: "icons/views/surface", built: false },
  { id: "tasks", label: "Tasks", key: "4", icon: "icons/views/tasks", built: false },
  { id: "findings", label: "Findings", key: "5", icon: "icons/views/findings", built: false },
];

export function Rail({ current }: { current: string }) {
  return (
    <nav className="rail" aria-label="Views">
      {VIEWS.map((v) =>
        v.built ? (
          <a
            key={v.id}
            href="/"
            data-view={v.id}
            className={v.id === current ? "on" : undefined}
            aria-current={v.id === current ? "page" : undefined}
            aria-label={v.label}
            title={`${v.label} (${v.key})`}
          >
            <Icon name={v.icon} />
          </a>
        ) : (
          <span
            key={v.id}
            data-view={v.id}
            className="todo"
            role="link"
            aria-disabled="true"
            aria-label={`${v.label} (not built yet)`}
            title={`${v.label} (${v.key}) · not built yet`}
          >
            <Icon name={v.icon} />
          </span>
        ),
      )}
    </nav>
  );
}
