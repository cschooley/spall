// Who made it (#62): `you`, or ◆ agent · T-3 linking to the task. Agent
// items carry the ◆ and a label, never color alone.
import { ofExchange, taskHref, type Attribution } from "../lib/attribution.ts";

/** "by you" / "by ◆ agent · T-3" with the task linked, for headers. */
export function By({ who, verb = "by", when }: { who: Attribution; verb?: string; when?: string }) {
  return (
    <span className={who.agent ? "by agent" : "by"}>
      {`${verb} `}
      {who.agent ? (
        who.task ? (
          <a href={taskHref(who.task)} title={`Open ${who.task} in Tasks`}>{`◆ ${who.label}`}</a>
        ) : (
          `◆ ${who.label}`
        )
      ) : (
        who.label
      )}
      {when ? ` ${when}` : null}
    </span>
  );
}

/** The ◆ on a Traffic row an agent made; nothing on yours. */
export function AgentMark({ meta }: { meta?: Record<string, string> }) {
  const who = ofExchange(meta);
  if (!who.agent) return null;
  return (
    <span className="by-agent" title={`made by ${who.label}`} aria-label={`made by ${who.label}`}>
      {`◆ ${who.task ?? "agent"}`}
    </span>
  );
}
