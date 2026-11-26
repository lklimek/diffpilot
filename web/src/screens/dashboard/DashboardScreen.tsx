import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router";
import { useDashboardTabStatus } from "../../app/tabStatus";
import { ApiError } from "../../api/client";
import { Banner } from "../../components/banner/Banner";
import { RefreshControl } from "../../components/dashboard/refresh-control/RefreshControl";
import { ScanProgress } from "../../components/dashboard/scan-progress/ScanProgress";
import { RefreshError } from "../../components/dashboard/refresh-error/RefreshError";
import { Headline, type ChartUnit } from "../../components/dashboard/headline/Headline";
import { WorkList } from "../../components/dashboard/work-list/WorkList";
import { decodeFilters, encodeFilters, type Filter } from "../../components/dashboard/filter-chips/filters";
import { getStatusCounts } from "../../components/dashboard/work-list/StatusFilter";
import { Mentions } from "../../components/dashboard/mentions/Mentions";
import { EmptyWork } from "../../components/dashboard/work-list/EmptyWork";
import { SidePanel } from "../../components/dashboard/side-panel/SidePanel";
import { ReadyToMerge } from "../../components/dashboard/ready-to-merge/ReadyToMerge";
import { WaitingOnOthers } from "../../components/dashboard/waiting-on-others/WaitingOnOthers";
import { Yesterday } from "../../components/dashboard/yesterday/Yesterday";
import { ThroughputMinis } from "../../components/dashboard/throughput-minis/ThroughputMinis";
import { QueueAge, ageBuckets } from "../../components/dashboard/queue-age/QueueAge";
import { ScanNotes } from "../../components/dashboard/scan-notes/ScanNotes";
import { KeyboardHelp } from "../../components/dashboard/keyboard-help/KeyboardHelp";
import { clockText } from "../../components/format/time";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import {
  barPullRequests,
  chartPoints,
  dailySeries,
  headline,
  throughputWeeks,
  weeklySeries,
} from "./history";
import { GITHUB_SYNC_SETTINGS_PATH, LLM_USAGE_PATH, PRS_PATH } from "../shell/paths";
import { ShellActions, useShellSlots } from "../shell/ShellRoute";
import { commentsHref, railText, rankingFeedback, reviewHref, watchErrorText } from "./state";
import { useDashboardPage } from "./useDashboardPage";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** Why a snooze failed: the item left the list (404), the server's reason, or a retry hint. */
const snoozeErrorText = (error: unknown) =>
  error instanceof ApiError && error.status === 404
    ? "Could not snooze this item: it is no longer in your list. Refresh to see the current list."
    : `Could not snooze this item. Try again. ${errorText(error)}`;

/** My work (plan §F, ADR-016): refresh state, headline and charts, the ranked list and the side panel. */
export function DashboardScreen() {
  const page = useDashboardPage();
  const navigate = useNavigate();
  const { pathname, search, hash } = useLocation();
  const [params, setParams] = useSearchParams();
  const filters = decodeFilters(params.get("f"));
  const setFilters = (filters: Filter[]) => {
    const next = new URLSearchParams(params);
    if (filters.length) next.set("f", encodeFilters(filters));
    else next.delete("f");
    void setParams(next, { preventScrollReset: true });
  };
  const go = (href: string) => void navigate(href);
  const [helpOpen, setHelpOpen] = useState(false);
  const [unit, setUnit] = useState<ChartUnit>("day");
  // The chart bar whose pull requests are listed, by its first day; it belongs to one unit.
  const [bar, setBar] = useState<{ unit: ChartUnit; day: string } | null>(null);
  const { me, user, view, now } = page;
  const summary = page.summary.data;
  const snapshot = summary?.snapshot ?? null;
  useDashboardTabStatus(summary?.latest_refresh);
  // The shell (layout route) shows My work's count, this page's help and its refresh control.
  const { setCount, setHelp } = useShellSlots();
  const count = snapshot ? (summary?.mentions ?? []).filter((m) => !m.is_bot).length : null;
  useEffect(() => setCount(count), [count, setCount]);
  const signedIn = user !== null;
  useEffect(() => {
    setHelp(signedIn ? () => setHelpOpen(true) : null);
    return () => setHelp(null);
  }, [signedIn, setHelp]);

  const zone = summary?.schedule.time_zone;
  const charts = useMemo(() => {
    if (!snapshot) return null;
    const events = snapshot.history;
    return {
      figures: headline(events, now, zone),
      days: chartPoints(dailySeries(events, now), "day"),
      weeks: chartPoints(weeklySeries(events, now), "week"),
      throughput: throughputWeeks(events, now),
    };
  }, [snapshot, now, zone]);

  const points = charts && (unit === "day" ? charts.days : charts.weeks);
  const selectedIndex = bar?.unit === unit ? (points?.findIndex((p) => p.day === bar.day) ?? -1) : -1;
  const listed = useMemo(() => {
    if (!snapshot || !bar) return null;
    const titles = new Map<string, string>();
    for (const item of [
      ...(page.allItems ?? []),
      ...snapshot.awaiting,
      ...(snapshot.exclusions?.items ?? []),
    ])
      if (item.repo && item.number !== null)
        titles.set(`${item.repo.owner}/${item.repo.name}#${item.number}`, item.title);
    return barPullRequests(snapshot.history, bar.day, bar.unit, titles);
  }, [snapshot, bar, page.allItems]);

  const snapshotAt = snapshot?.scanned_at ?? null;
  const stale = view?.stale && snapshotAt ? clockText(snapshotAt, now) : null;
  const actionErrors = [page.refresh.error, page.cancel.error, page.retryRanking.error].filter(
    (error): error is string => error !== null,
  );
  const ranking = summary && rankingFeedback(summary, page.snapshotRefresh);
  const allItems = page.allItems;
  const retryId = ranking?.retryId;
  const retryRanking = retryId ? () => page.retryRanking.run(retryId) : undefined;
  const counts =
    summary?.status_counts ??
    getStatusCounts(allItems ?? [], snapshot?.exclusions?.total ?? snapshot?.excluded ?? 0);
  const scan = view?.scanning;
  const cancelScan = scan ? () => page.cancel.run(scan.id) : undefined;
  const live =
    summary && summary.live_repositories !== undefined && summary.not_live_repositories !== undefined
      ? {
          live_repositories: summary.live_repositories,
          not_live_repositories: summary.not_live_repositories,
          last_event_at: summary.last_event_at,
        }
      : undefined;

  const main = view && summary && (
    <div className="grid content-start gap-3.5">
      <div>
        <h1 className="text-[22px] font-bold">My work</h1>
        <p className="text-muted">
          PRs, issues and TODOs that need you, ranked by score. Read-only: this page never writes to GitHub.
        </p>
      </div>
      {actionErrors.map((text) => (
        <Banner key={text} tone="error" title={text} />
      ))}
      {view.failure && (
        <RefreshError
          status={view.failure}
          snapshotAt={snapshotAt}
          onRetry={page.refresh.start}
          retrying={page.refresh.starting}
          now={now}
        />
      )}
      {view.notes.map((note) => (
        <Banner key={note.id} tone={note.tone} title={note.title}>
          {note.text}
        </Banner>
      ))}
      {!snapshot && !view.scanning && (
        <Banner tone="info" title="Nothing scanned yet.">
          Refresh reads the pull requests, issues and TODOs that need you from GitHub and MemCan (about 1–2
          minutes), then the LLM judges new items.
        </Banner>
      )}
      {snapshot && charts && (
        <Headline
          figures={charts.figures}
          zoneLabel={charts.figures.zoneLabel}
          unsupportedZone={charts.figures.unsupportedZone}
          days={charts.days}
          weeks={charts.weeks}
          unit={unit}
          onUnitChange={setUnit}
          selected={selectedIndex >= 0 ? selectedIndex : null}
          onSelect={(i) => setBar(i === null || !points?.[i] ? null : { unit, day: points[i].day })}
          listed={listed}
          staleNote={stale && `From the ${stale} snapshot.`}
        />
      )}
      <Mentions
        items={summary.mentions ?? []}
        coverage={summary.mention_coverage}
        withBots={page.withBots}
        onWithBotsChange={page.setWithBots}
        onHide={(id) => page.hideMention.mutate(id)}
        hidingIds={page.hideMention.pendingIds}
        expanded={page.mentionsExpanded}
        onExpandedChange={page.setMentionsExpanded}
        now={now}
      />
      {page.hideMention.isError && (
        <StatusMessage kind="error">
          Could not hide this mention. Try again. {errorText(page.hideMention.error)}{" "}
          <Tooltip tip="Dismiss this error message">
            <button type="button" className="underline" onClick={page.hideMention.dismissError}>
              Dismiss
            </button>
          </Tooltip>
        </StatusMessage>
      )}
      {page.snooze.isError && (
        <StatusMessage kind="error">
          {snoozeErrorText(page.snooze.error)}{" "}
          <Tooltip tip="Dismiss this error message">
            <button type="button" className="underline" onClick={page.snooze.dismissError}>
              Dismiss
            </button>
          </Tooltip>
        </StatusMessage>
      )}
      {page.watch.failure && (
        <StatusMessage kind="error">
          {watchErrorText(page.watch.failure.error, page.watch.failure.watching)}{" "}
          <Tooltip tip="Dismiss this error message">
            <button type="button" className="underline" onClick={page.watch.dismissError}>
              Dismiss
            </button>
          </Tooltip>
        </StatusMessage>
      )}
      {page.wake.isError && (
        <StatusMessage kind="error">
          Could not wake this item. Try again. {errorText(page.wake.error)}{" "}
          <Tooltip tip="Dismiss this error message">
            <button type="button" className="underline" onClick={page.wake.dismissError}>
              Dismiss
            </button>
          </Tooltip>
        </StatusMessage>
      )}
      {page.rejudge.error && (
        <StatusMessage kind="error">
          Could not re-judge this item. {errorText(page.rejudge.error)}{" "}
          <Tooltip tip="Dismiss this error message">
            <button type="button" className="min-h-6 underline" onClick={page.rejudge.dismissError}>
              Dismiss
            </button>
          </Tooltip>
        </StatusMessage>
      )}
      {scan && cancelScan && (
        <ScanProgress
          status={scan}
          snapshotAt={snapshotAt}
          onCancel={cancelScan}
          cancelling={page.cancel.pending}
          now={now}
        />
      )}
      {snapshot && !view.empty && page.items.isPending && (
        <StatusMessage kind="loading">Loading the list…</StatusMessage>
      )}
      {snapshot && !view.empty && page.items.isError && (
        <StatusMessage kind="error">{errorText(page.items.error)}</StatusMessage>
      )}
      {snapshot && (view.empty || allItems) && (
        <WorkList
          guideOrigin={pathname + search + hash}
          items={view.empty ? [] : (allItems ?? [])}
          kinds={page.filter}
          onKindsChange={page.setFilter}
          filters={filters}
          onFiltersChange={setFilters}
          statusCounts={counts}
          statusDescriptions={ranking?.statusDescriptions}
          exclusions={snapshot.exclusions}
          onWake={(key) => page.wake.mutate({ key })}
          wakingKeys={page.wake.pendingKeys}
          onRejudge={(key) => page.rejudge.mutate({ key })}
          rejudgingKeys={page.rejudge.pendingKeys}
          usageHref={LLM_USAGE_PATH}
          snapshotAt={snapshotAt}
          reviewHref={reviewHref}
          commentsHref={commentsHref}
          onWatch={(item, watching) =>
            item.repo &&
            item.number !== null &&
            page.watch.mutate({ key: item.key, watching, pr: { repo: item.repo, number: item.number } })
          }
          watchingKeys={page.watch.pendingKeys}
          onNavigate={go}
          onHelp={() => setHelpOpen(true)}
          keysEnabled={!helpOpen}
          stale={stale ? { at: stale, note: "It is replaced when a scan finishes." } : null}
          todos={snapshot.todos}
          empty={
            view.empty && (
              <EmptyWork
                checkedAt={snapshot.scanned_at}
                checked={snapshot.checked}
                awaiting={snapshot.awaiting.length}
                hasMentions={(summary.mentions ?? []).length > 0}
                reviewStartHref={PRS_PATH}
                onNavigate={go}
                now={now}
              />
            )
          }
          onSnooze={(item, days) => page.snooze.mutate({ key: item.key, days })}
          snoozingKeys={page.snooze.pendingKeys}
          now={now}
        />
      )}
    </div>
  );

  return (
    <>
      {summary && (
        <ShellActions>
          <RefreshControl
            latest={summary.latest_refresh}
            snapshot={snapshot}
            schedule={summary.schedule}
            snapshotRefresh={page.snapshotRefresh}
            live={live}
            outdated={counts.outdated}
            onRejudgeOutdated={summary.status_counts ? page.refresh.rejudgeOutdated : undefined}
            onCancel={cancelScan}
            cancelling={page.cancel.pending}
            rankingNotices={ranking?.notices}
            onRetryRanking={retryRanking}
            retryingRanking={page.retryRanking.pending}
            onRefresh={page.refresh.start}
            starting={page.refresh.starting}
            now={now}
          />
        </ShellActions>
      )}
      {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
      {me.isError && <StatusMessage kind="error">{errorText(me.error)}</StatusMessage>}
      {me.isSuccess && !user && (
        <div className="max-w-[900px]">
          <SignInCard loginHref="/auth/github/login" />
        </div>
      )}
      {user && page.summary.isPending && <StatusMessage kind="loading">Loading your work…</StatusMessage>}
      {user && page.summary.isError && (
        <StatusMessage kind="error">{errorText(page.summary.error)}</StatusMessage>
      )}
      {main &&
        (snapshot && charts ? (
          <SidePanel main={main} railText={railText(snapshot)}>
            <WaitingOnOthers items={snapshot.awaiting} now={now} slackNames={page.slackNames} />
            <ReadyToMerge
              items={snapshot.ready_to_merge}
              pending={snapshot.merge_state_pending}
              plugins={summary?.plugins}
            />
            <Yesterday
              label={charts.figures.yesterday.label}
              zoneLabel={charts.figures.zoneLabel}
              prs={charts.figures.yesterdayDetails.prs}
              reviews={charts.figures.yesterdayDetails.reviews}
            />
            <ThroughputMinis weeks={charts.throughput} />
            <QueueAge
              yoursWaiting={ageBuckets(
                snapshot.awaiting.map((item) => item.waiting_since),
                now,
              )}
              // TODO(WP11-WEB-S): the contract has no review-request age per item; "waiting on you" stays unknown.
              waitingOnYou={null}
            />
            <ScanNotes
              notes={snapshot.notes}
              unreachable={snapshot.unreachable}
              todos={snapshot.todos}
              settingsHref={GITHUB_SYNC_SETTINGS_PATH}
              onNavigate={go}
              now={now}
            />
          </SidePanel>
        ) : (
          main
        ))}
      <p
        role="status"
        className={
          page.notice
            ? "fixed right-4 bottom-4 z-40 rounded-md border border-border bg-surface px-3 py-2 shadow"
            : "sr-only"
        }
      >
        {page.notice}
      </p>
      <KeyboardHelp open={helpOpen} onClose={() => setHelpOpen(false)} />
    </>
  );
}
