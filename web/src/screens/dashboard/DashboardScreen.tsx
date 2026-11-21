import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useDashboardTabStatus } from "../../app/tabStatus";
import { ApiError } from "../../api/client";
import { Banner } from "../../components/banner/Banner";
import { RefreshControl } from "../../components/dashboard/refresh-control/RefreshControl";
import { ScanProgress } from "../../components/dashboard/scan-progress/ScanProgress";
import { RankingBanner } from "../../components/dashboard/ranking-banner/RankingBanner";
import { RefreshError } from "../../components/dashboard/refresh-error/RefreshError";
import { Headline, type ChartUnit } from "../../components/dashboard/headline/Headline";
import { WorkList } from "../../components/dashboard/work-list/WorkList";
import { ExcludedItems } from "../../components/dashboard/excluded-items/ExcludedItems";
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
  fullDayLabel,
  headline,
  throughputWeeks,
  weeklySeries,
  yesterday,
} from "./history";
import { REVIEW_START_PATH, SETTINGS_PATH } from "../shell/paths";
import { ShellActions, useShellSlots } from "../shell/ShellRoute";
import { railText, reviewHref } from "./state";
import { useDashboardPage } from "./useDashboardPage";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** My work (plan §F, ADR-016): refresh state, headline and charts, the ranked list and the side panel. */
export function DashboardScreen() {
  const page = useDashboardPage();
  const navigate = useNavigate();
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

  // Charts are bucketed by the browser's local day and week (plan §C.1).
  const charts = useMemo(() => {
    if (!snapshot) return null;
    const events = snapshot.history;
    return {
      figures: headline(events, now),
      days: chartPoints(dailySeries(events, now), "day"),
      weeks: chartPoints(weeklySeries(events, now), "week"),
      throughput: throughputWeeks(events, now),
      yesterday: yesterday(events, now),
    };
  }, [snapshot, now]);

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
  const excluded = snapshot && <ExcludedItems exclusions={snapshot.exclusions} onNavigate={go} />;
  const stale = view?.stale && snapshotAt ? clockText(snapshotAt, now) : null;
  const actionErrors = [page.refresh.error, page.cancel.error, page.rankAgain.error].filter(
    (e): e is string => e !== null,
  );

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
      {view.scanning && (
        <ScanProgress
          status={view.scanning}
          snapshotAt={snapshotAt}
          onCancel={() => page.cancel.run(view.scanning!.id)}
          cancelling={page.cancel.pending}
          now={now}
        />
      )}
      {view.failure && (
        <RefreshError
          status={view.failure}
          snapshotAt={snapshotAt}
          onRetry={page.refresh.start}
          retrying={page.refresh.starting}
          now={now}
        />
      )}
      {view.ranking && (
        <RankingBanner
          ranking={view.ranking.status}
          scannedAt={view.ranking.scannedAt}
          onRetry={() => page.rankAgain.run(view.ranking!.refreshId)}
          retrying={page.rankAgain.pending}
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
          minutes), then the LLM ranks them.
        </Banner>
      )}
      {snapshot && charts && (
        <Headline
          figures={charts.figures}
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
      {snapshot && view.empty && (
        <EmptyWork
          checkedAt={snapshot.scanned_at}
          checked={snapshot.checked}
          excluded={excluded}
          awaiting={snapshot.awaiting.length}
          hasMentions={(summary.mentions ?? []).length > 0}
          reviewStartHref={REVIEW_START_PATH}
          onNavigate={go}
          now={now}
        />
      )}
      {snapshot && !view.empty && page.items.isPending && (
        <StatusMessage kind="loading">Loading the list…</StatusMessage>
      )}
      {snapshot && !view.empty && page.items.isError && (
        <StatusMessage kind="error">{errorText(page.items.error)}</StatusMessage>
      )}
      {snapshot && !view.empty && page.allItems && (
        <WorkList
          items={page.allItems}
          kinds={page.filter}
          onKindsChange={page.setFilter}
          sizes={page.sizes}
          onSizesChange={page.setSizes}
          reviewHref={reviewHref}
          onNavigate={go}
          onHelp={() => setHelpOpen(true)}
          keysEnabled={!helpOpen}
          stale={stale ? { at: stale, note: "It is replaced when a scan finishes." } : null}
          todos={snapshot.todos}
          notice={excluded}
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
            <WaitingOnOthers items={snapshot.awaiting} now={now} />
            <ReadyToMerge
              items={snapshot.ready_to_merge}
              pending={snapshot.merge_state_pending}
              held={snapshot.ready_to_merge_held}
            />
            <Yesterday
              label={fullDayLabel(charts.yesterday.day)}
              prs={charts.yesterday.prs}
              reviews={charts.yesterday.reviews}
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
              settingsHref={SETTINGS_PATH}
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
