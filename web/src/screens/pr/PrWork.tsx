import { useState } from "react";
import { useNavigate } from "react-router";
import { errorText } from "../../api/client";
import type { HistoryKind, PrRef, RepoRef } from "../../api/generated/types";
import { AlarmRow } from "../../components/dashboard/alarm-row/AlarmRow";
import { ExcludedItems } from "../../components/dashboard/excluded-items/ExcludedItems";
import { Mentions } from "../../components/dashboard/mentions/Mentions";
import { ReadyToMerge } from "../../components/dashboard/ready-to-merge/ReadyToMerge";
import { WaitingOnOthers } from "../../components/dashboard/waiting-on-others/WaitingOnOthers";
import { WorkItemRow } from "../../components/dashboard/work-item/WorkItemRow";
import { dateTimeText } from "../../components/format/time";
import { StatusMessage } from "../../components/status/StatusMessage";
import { useMentionBots } from "../dashboard/mentions";
import { useDashboardItems, useDashboardSummary, useHideMention, useSlackNames } from "../dashboard/queries";
import { WorkItemPathLinks } from "../dashboard/WorkItemPathLinks";
import { LLM_USAGE_PATH } from "../shell/paths";
import { commentsPath, prPath, samePr } from "./paths";

const EVENT_LABEL: Record<HistoryKind, string> = {
  pr_opened: "Opened",
  pr_merged: "Merged",
  pr_closed_unmerged: "Closed without merge",
  review_given: "Review given",
};

function matches(pr: PrRef, item: { repo: RepoRef | null; number: number | null }) {
  return item.repo !== null && item.number !== null && samePr(pr, { repo: item.repo, number: item.number });
}

/** Snapshot details My work shows for this PR, using the same reader's data and presentation. */
export function PrWork({ pr, now }: { pr: PrRef; now: Date }) {
  const navigate = useNavigate();
  const go = (href: string) => void navigate(href);
  const summary = useDashboardSummary(true);
  const snapshot = summary.data?.snapshot;
  const items = useDashboardItems(summary.data?.revision, snapshot != null);
  const slackNames = useSlackNames(Boolean(snapshot?.awaiting.some((item) => matches(pr, item))));
  const hide = useHideMention();
  const [withBots, setWithBots] = useMentionBots();
  const [mentionsExpanded, setMentionsExpanded] = useState(true);
  const [expanded, setExpanded] = useState(true);

  const entry = items.data?.items.find((item) => item.kind === "pr" && matches(pr, item));
  const mentions = summary.data?.mentions.filter((item) => item.kind === "pr" && matches(pr, item)) ?? [];
  const awaiting = snapshot?.awaiting.filter((item) => matches(pr, item)) ?? [];
  const ready = snapshot?.ready_to_merge.filter((item) => matches(pr, item)) ?? [];
  const excluded = snapshot?.exclusions?.items?.find((item) => item.kind === "pr" && matches(pr, item));
  const id = `${pr.repo.owner}/${pr.repo.name}#${pr.number}`.toLowerCase();
  const events = snapshot?.history.filter((event) => event.item_id.toLowerCase() === id) ?? [];
  const error = summary.error ?? items.error ?? hide.error;
  const ranking = entry && {
    item: entry,
    expanded,
    onToggle: () => setExpanded((open) => !open),
    snapshotAt: snapshot?.scanned_at,
    commentsHref: commentsPath(pr),
    watched: entry.watched,
    usageHref: LLM_USAGE_PATH,
    onNavigate: go,
    now,
  };

  return (
    <section aria-labelledby="pr-work-heading" className="grid min-w-0 gap-2">
      <h2 id="pr-work-heading" className="text-base font-semibold">
        My work
      </h2>
      {(summary.isPending || (snapshot && items.isPending)) && (
        <StatusMessage kind="loading">Loading your work…</StatusMessage>
      )}
      {error && <StatusMessage kind="error">{errorText(error)}</StatusMessage>}
      {ranking && (
        <WorkItemPathLinks item={ranking.item} expanded={expanded}>
          {ranking.item.score.tier === "p0" ? (
            <AlarmRow {...ranking} />
          ) : (
            <ul aria-label="Ranking" className="min-w-0 list-none rounded-lg border border-border bg-surface">
              <WorkItemRow {...ranking} />
            </ul>
          )}
        </WorkItemPathLinks>
      )}
      <Mentions
        items={mentions}
        withBots={withBots}
        onWithBotsChange={setWithBots}
        expanded={mentionsExpanded}
        onExpandedChange={setMentionsExpanded}
        onHide={(mention) => hide.mutate(mention)}
        hidingIds={hide.pendingIds}
        now={now}
      />
      {awaiting.length > 0 && <WaitingOnOthers items={awaiting} now={now} slackNames={slackNames.data} />}
      {ready.length > 0 && <ReadyToMerge items={ready} pending={0} plugins={summary.data?.plugins} />}
      {excluded && (
        <ExcludedItems
          exclusions={{ total: 1, counts: [{ reason: excluded.reason, count: 1 }], items: [excluded] }}
          guideOrigin={prPath(pr)}
          onNavigate={go}
        />
      )}
      {events.length > 0 && (
        <div className="rounded-lg border border-border bg-surface p-3">
          <h3 className="text-sm font-semibold">Activity</h3>
          <ul className="text-sm text-muted">
            {events.map((event) => (
              <li key={event.kind}>
                {EVENT_LABEL[event.kind]} · <time dateTime={event.at}>{dateTimeText(event.at)}</time>
              </li>
            ))}
          </ul>
        </div>
      )}
      {!error &&
        summary.isSuccess &&
        (!snapshot || items.isSuccess) &&
        !entry &&
        !mentions.length &&
        !awaiting.length &&
        !ready.length &&
        !excluded &&
        !events.length && (
          <p className="text-sm text-muted">This pull request is not in your current My work snapshot.</p>
        )}
    </section>
  );
}
