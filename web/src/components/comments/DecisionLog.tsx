import type { CommentLog, Decision, Effect, EffectState, FeedSubscriber } from "../../api/generated/types";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { clockText, dateTimeText } from "../format/time";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { CHOICE_LABEL, decisionLabel, EFFECT_WORD } from "./decisions";
import { Chip, FactChip } from "./parts";

const STATE: Record<EffectState, string> = {
  waiting: "waiting for the step before",
  open: "open",
  claimed: "claimed",
  done: "done",
  failed: "failed",
  cancelled: "cancelled",
};

const VIA: Record<Decision["via"], string> = {
  question: "in Questions",
  group: "in a group",
  bulk: 'via "decide the rest"',
  table: "via table",
  carried: "carried",
};

const TH = "border-b border-border px-2 py-1 text-left text-[11px] font-semibold text-muted uppercase";
const TD = "border-b border-border px-2 py-1 align-top";

/** "Agent build-host-2 reports: done (link)" and GitHub facts of one effect. */
function EffectLine({ effect, now }: { effect: Effect; now: Date }) {
  const report = effect.reports.at(-1);
  return (
    <li>
      {EFFECT_WORD[effect.kind] ?? effect.kind}: {STATE[effect.state] ?? effect.state}
      {effect.claim &&
        ` by ${effect.claim.token_name} (lease until ${clockText(effect.claim.lease_until, now)})`}
      {report && (
        <>
          {" · "}Agent {report.token_name} reports: {report.outcome}
          {report.message && ` (${report.message})`}
          {report.reference && (
            <>
              {" "}
              {report.reference.startsWith("https://") ? (
                <OpenExternal
                  href={report.reference}
                  label={`Open the reported ${EFFECT_WORD[effect.kind]}`}
                />
              ) : (
                <code>{report.reference.slice(0, 12)}</code>
              )}
            </>
          )}
        </>
      )}
      {effect.seen_on_github_at && (
        <>
          {" "}
          <FactChip
            text={`Found on GitHub · ${clockText(effect.seen_on_github_at, now)}`}
            tip="A GitHub read by diffpilot found this reply, comment or resolve."
          />
        </>
      )}
    </li>
  );
}

function Agents({ subscribers, now }: { subscribers: FeedSubscriber[]; now: Date }) {
  if (subscribers.length === 0)
    return (
      <p className="m-0 text-sm">
        No agent subscribed: decided effects stay open until an agent claims them.
      </p>
    );
  return (
    <table className="w-full border-collapse text-[13px]">
      <caption className="sr-only">Agents subscribed to the decision feed</caption>
      <thead>
        <tr>
          {["Agent (token name)", "Capabilities", "Connected", "Last seen", "Push target"].map((h) => (
            <th key={h} scope="col" className={TH}>
              {h}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {subscribers.map((s) => (
          <tr key={s.token_name}>
            <td className={TD}>{s.token_name}</td>
            <td className={TD}>{s.capabilities.map((c) => EFFECT_WORD[c] ?? c).join(", ")}</td>
            <td className={TD}>{s.connected ? "event stream" : "not connected"}</td>
            <td className={TD}>{clockText(s.last_seen_at, now)}</td>
            <td className={TD}>
              {!s.push ? (
                <span className="text-muted">none{s.connected ? " (connected; not pushed)" : ""}</span>
              ) : (
                <>
                  {s.push.kind} · {s.push.url}
                  {s.push.topic && `/${s.push.topic}`}{" "}
                  {s.push.delivery.failing_since ? (
                    <Chip
                      tone="fail"
                      text={`failing since ${clockText(s.push.delivery.failing_since, now)}: ${s.push.delivery.last_error ?? "error"}`}
                      tip="Delivery state of this push target, as diffpilot recorded it."
                    />
                  ) : s.push.delivery.last ? (
                    <FactChip
                      text={`delivered ${clockText(s.push.delivery.last.at, now)}`}
                      tip="Latest delivery to this push target, as diffpilot recorded it."
                    />
                  ) : null}
                </>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** V3: agents and push targets, open and failed effects, and every decision with its effects and reports. */
export function DecisionLog({ log, now, storageKey }: { log: CommentLog; now: Date; storageKey: string }) {
  return (
    <div>
      <CollapsibleSection title="Decision feed" level={2} storageKey={`${storageKey}:feed`}>
        <div className="rounded-lg border border-border bg-surface px-3 py-2">
          <Agents subscribers={log.subscribers} now={now} />
          <p className="mt-1.5 mb-0 text-xs">
            Each agent sets its own push target with its access token. A push names only the decision and the
            PR with a link to fetch it through the API; no reply text, reasons, comments or tokens. Agents
            with a live event stream are not pushed.
          </p>
          <p className="mt-1 mb-0 text-xs">
            Open effects: {log.open_effects} · failed: {log.failed_effects}
          </p>
        </div>
      </CollapsibleSection>
      <CollapsibleSection title="Decisions" level={2} storageKey={`${storageKey}:decisions`}>
        {log.decisions.length === 0 ? (
          <p className="text-muted">No decision recorded through diffpilot yet.</p>
        ) : (
          <ol className="m-0 grid list-none gap-2 p-0 text-[13px]">
            {[...log.decisions].reverse().map((d) => (
              <li key={d.id} className="rounded-lg border border-border bg-surface px-3 py-2">
                <b>
                  {dateTimeText(d.decided_at)} · C{d.item_no} · {decisionLabel(d)}
                </b>{" "}
                by {d.decided_by} · {VIA[d.via] ?? d.via} · resolve: {d.resolve_ticked ? "yes" : "no"}
                {d.recommendation && (
                  <>
                    {" "}
                    · recommendation then: {CHOICE_LABEL[d.recommendation.kind] ?? d.recommendation.kind}{" "}
                    <ProvenanceTag source="llm" />
                  </>
                )}
                {d.reason && <div>Reason posted: &quot;{d.reason}&quot;</div>}
                {d.comment && (
                  <div>Your comment (sent to the agent, not posted): &quot;{d.comment}&quot;</div>
                )}
                {d.effects.length > 0 && (
                  <ul className="m-0 mt-1 pl-5">
                    {d.effects.map((e) => (
                      <EffectLine key={e.id} effect={e} now={now} />
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ol>
        )}
      </CollapsibleSection>
    </div>
  );
}
