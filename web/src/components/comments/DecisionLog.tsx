import type {
  CommentLog,
  Decision,
  DecisionKind,
  Effect,
  EffectState,
  FeedSubscriber,
  PrRef,
} from "../../api/generated/types";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { clockText, dateTimeText } from "../format/time";
import { commitGithubUrl, isGithubUrl } from "../github/links";
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

/** Decisions on PR-level questions (`P<n>`, as Questions names them). */
const PR_LEVEL: ReadonlySet<DecisionKind> = new Set([
  "merge_with_follow_ups",
  "another_round",
  "dismiss_stale_reviews",
  "keep_reviews",
]);

/** `C14`, or `P24` for a decision on a PR-level question. */
const decisionItemId = (d: Pick<Decision, "item_no" | "kind">) =>
  `${PR_LEVEL.has(d.kind) ? "P" : "C"}${d.item_no}`;

/**
 * An agent's reference (agents read untrusted PR comments, threat model T24): a GitHub URL becomes a link, a commit
 * id a link to that commit of the PR's repository; anything else stays text.
 */
function Reference({ reference, effect, pr }: { reference: string; effect: Effect; pr: PrRef }) {
  if (isGithubUrl(reference))
    return <OpenExternal href={reference} label={`Open the reported ${EFFECT_WORD[effect.kind]}`} />;
  const commit = commitGithubUrl(pr, reference);
  if (commit)
    return (
      <>
        <code>{reference.slice(0, 7)}</code>{" "}
        <OpenExternal href={commit} label={`Open commit ${reference.slice(0, 7)} on GitHub`} />
      </>
    );
  return <code className="break-all">{reference}</code>;
}

/** "Agent build-host-2 reports: done (link)" and GitHub facts of one effect. */
function EffectLine({ effect, now, pr }: { effect: Effect; now: Date; pr: PrRef }) {
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
              <Reference reference={report.reference} effect={effect} pr={pr} />
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
                  {s.push.kind}
                  {s.push.url === null ? (
                    // The server leaves out another user's address: a URL or topic can be a credential.
                    <span className="text-muted"> · another user's token, address not shown</span>
                  ) : (
                    ` · ${s.push.url}`
                  )}
                  {s.push.topic && `/${s.push.topic}`}{" "}
                  {s.push.delivery.failing_since ? (
                    <Chip
                      tone="fail"
                      text={`failing since ${clockText(s.push.delivery.failing_since, now)}${s.push.delivery.last_error ? `: ${s.push.delivery.last_error}` : ""}`}
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

interface DecisionLogProps {
  log: CommentLog;
  /** The pull request (commit links of agent reports). */
  pr: PrRef;
  now: Date;
  storageKey: string;
}

/**
 * V3: agents and push targets, open and failed effects, and every decision with its effects and reports; an undone
 * decision says so.
 */
export function DecisionLog({ log, pr, now, storageKey }: DecisionLogProps) {
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
              <li
                key={d.id}
                className={`rounded-lg border border-border bg-surface px-3 py-2 ${d.undone_at ? "text-muted" : ""}`}
              >
                <b className={d.undone_at ? "line-through" : undefined}>
                  {dateTimeText(d.decided_at)} · {decisionItemId(d)} · {decisionLabel(d)}
                </b>{" "}
                {d.undone_at && (
                  <>
                    <b>Undone {dateTimeText(d.undone_at)}</b>{" "}
                  </>
                )}
                by {d.decided_by} · {VIA[d.via] ?? d.via} · resolve: {d.resolve_ticked ? "yes" : "no"}
                {d.recommendation && (
                  <>
                    {" "}
                    · recommendation then: {CHOICE_LABEL[d.recommendation.kind] ?? d.recommendation.kind}{" "}
                    <ProvenanceTag source="llm" />
                  </>
                )}
                {d.reply_text && <div>Public reply: &quot;{d.reply_text}&quot;</div>}
                {d.reason && !d.reply_text?.includes(d.reason) && (
                  <div>Reason (not posted): &quot;{d.reason}&quot;</div>
                )}
                {d.comment && (
                  <div>Your comment (sent to the agent, not posted): &quot;{d.comment}&quot;</div>
                )}
                {d.effects.length > 0 && (
                  <ul className="m-0 mt-1 pl-5">
                    {d.effects.map((e) => (
                      <EffectLine key={e.id} effect={e} now={now} pr={pr} />
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
