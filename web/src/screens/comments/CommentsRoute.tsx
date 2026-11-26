import {
  useAssessmentSettings,
  useSetAssessmentSettings,
  useAssessmentComparison,
} from "../../api/assessment-settings";
import { AssessmentControls } from "../../components/comments/AssessmentControls";
import { AssessmentComparisonPanel } from "../../components/comments/AssessmentComparisonReport";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import {
  Link,
  Outlet,
  useLocation,
  useNavigate,
  useOutletContext,
  useParams,
  useSearchParams,
} from "react-router";
import { ApiError, errorText } from "../../api/client";
import type { CommentItem, PrRef, CommentsOverview } from "../../api/generated/types";
import { useMe } from "../../api/queries";
import {
  useCommentItems,
  useCommentsOverview,
  useImportedCommentItems,
  usePrPage,
  usePrQuestions,
  useReadComments,
  useStartCommentAssessment,
  useVisitComments,
  useWatchPr,
  type AssessmentStartIssue,
} from "../../api/review-loop";
import { useNow } from "../../app/now";
import { usePrTabSubject } from "../../app/tabStatus";
import { busyTip, buttonClass } from "../../components/button/button";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { Banner } from "../../components/banner/Banner";
import { ImportedDecisions } from "../../components/comments/ImportedDecisions";
import { AssessmentBanner } from "../../components/comments/AssessmentBanner";
import { OpenExternal } from "../../components/dashboard/common/OpenExternal";
import { WatchControl } from "../../components/watch/WatchControl";
import { CommentsTabs, type CommentsPage } from "../../components/comments/CommentsTabs";
import { READ_FAILURE } from "../../components/comments/parts";
import { StatusPopover } from "../../components/status/StatusPopover";
import { clockText, dateTimeText } from "../../components/format/time";
import { StatusMessage } from "../../components/status/StatusMessage";
import { prGithubUrl } from "../../components/github/links";
import { ReviewTopBar } from "../shell/ReviewTopBar";
import { AnalysisPathLinks } from "../review/AnalysisPathLinks";
import { PrAskChatProvider } from "../review/askChat";
import { ACCOUNT_SETTINGS_PATH } from "../shell/paths";
import { commentsPath, prOfParams, prPath } from "../pr/paths";

/** What every Comments page gets from the layout. */
export interface CommentsContext {
  pr: PrRef;
  overview: CommentsOverview;
  items: CommentItem[];
  needsYou: number;
  statusHost: RefObject<HTMLDivElement | null>;
  focusQueueStatus: () => void;
  /** Ask answers about an item (stored count, or the live chat's, whichever is higher). */
  askAnswers: (item: CommentItem) => number;
  login: string;
  now: Date;
}

/** The layout's data for a Comments page. */
export const useComments = () => useOutletContext<CommentsContext>();

/** The shared Ask chat over the reader's questions about the PR's comment items, with the live answer counts. */
function WithAsk({
  pr,
  items,
  children,
}: {
  pr: PrRef;
  items: readonly CommentItem[] | undefined;
  children: (count: (id: string) => number) => ReactNode;
}) {
  const questions = usePrQuestions(pr);
  const numbers = useMemo(() => new Map((items ?? []).map((item) => [item.id, item.no])), [items]);
  const count = useCallback(
    (id: string) =>
      (questions.data ?? []).filter(
        (q) => q.state === "answered" && q.anchor.kind === "comment_item" && q.anchor.item === id,
      ).length,
    [questions.data],
  );
  return (
    <PrAskChatProvider key={commentsPath(pr)} pr={pr} numbers={numbers}>
      {children(count)}
    </PrAskChatProvider>
  );
}

const PAGE_OF: [string, CommentsPage][] = [
  ["/table", "table"],
  ["/rounds", "rounds"],
  ["/carry-overs", "carry-overs"],
  ["/log", "log"],
];

/** Layout of V1–V5: top bar with the view menu, the PR status, the Comments pages and the shared Ask chat. */
export function CommentsRoute() {
  const pr = prOfParams(useParams());
  return <CommentsVisit key={commentsPath(pr)} pr={pr} />;
}

const visitUnavailable = (error: unknown) => error instanceof ApiError && error.status >= 500;
const visitUnconfirmed = (error: unknown) =>
  visitUnavailable(error) || (error instanceof ApiError && error.status === 0);

function CommentsVisit({ pr }: { pr: PrRef }) {
  const { pathname } = useLocation();
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const me = useMe();
  const login = me.data?.user?.login ?? "anonymous";
  const { mutate: recordVisit, isPending: recordingVisit, error: visitError } = useVisitComments(pr);
  const [canRead, setCanRead] = useState(false);
  const [assessmentIssue, setAssessmentIssue] = useState<AssessmentStartIssue | null>(null);
  const entryAssessment = useStartCommentAssessment(pr);
  const startAssessment = entryAssessment.mutate;
  const entered = useRef(false);
  const visit = useCallback(() => {
    const enter = () => {
      if (canRead) return;
      setCanRead(true);
      startAssessment(undefined, { onSuccess: setAssessmentIssue });
    };
    recordVisit(undefined, {
      onSuccess: enter,
      onError: (error) => {
        if (visitUnconfirmed(error)) enter();
      },
    });
  }, [canRead, recordVisit, startAssessment]);
  useEffect(() => {
    if (entered.current) return;
    entered.current = true;
    visit();
  }, [visit]);
  const uncertain = visitUnconfirmed(visitError);
  const retryingVisit = canRead && recordingVisit;
  const ready = canRead && entryAssessment.isSuccess;
  const overview = useCommentsOverview(pr, ready);
  // Load items after the initial overview identifies the stored PR.
  const overviewRead = ready && overview.isFetchedAfterMount;
  const items = useCommentItems(pr, overviewRead);
  const showImports = search.get("imported_unseen") === "true";
  const imports = useImportedCommentItems(pr, overviewRead && showImports);
  const page = usePrPage(pr);
  const read = useReadComments(pr, setAssessmentIssue);
  const watch = useWatchPr(pr);
  const statusHost = useRef<HTMLDivElement>(null);
  const activeTab = useRef<HTMLAnchorElement>(null);
  const focusStatusOnMount = useRef(false);
  const focusQueueStatus = () => {
    if (!focusStatusOnMount.current) return;
    const button = statusHost.current?.querySelector("button");
    if (!button) return;
    focusStatusOnMount.current = false;
    button.focus();
  };
  const assessmentSettings = useAssessmentSettings();
  const saveAssessment = useSetAssessmentSettings();
  const [showComparison, setShowComparison] = useState(false);
  const comparison = useAssessmentComparison(pr, showComparison);
  const now = new Date(useNow());
  usePrTabSubject(pr);
  const current = PAGE_OF.find(([suffix]) => pathname.endsWith(suffix))?.[1] ?? "questions";
  const go = useCallback((href: string) => void navigate(href), [navigate]);
  const hrefs = useMemo(
    () =>
      Object.fromEntries(
        (["questions", "table", "rounds", "carry-overs", "log"] as const).map((p) => [
          p,
          commentsPath(pr, p),
        ]),
      ) as Record<CommentsPage, string>,
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the PR's parts are its identity
    [pr.repo.owner, pr.repo.name, pr.number],
  );
  const o = overviewRead ? overview.data : undefined;
  const needsYou = o?.counts.needs_you ?? 0;
  const unseen = o?.counts.imported_unseen ?? 0;
  const importSearch = new URLSearchParams(search);
  if (showImports) importSearch.delete("imported_unseen");
  else importSearch.set("imported_unseen", "true");
  const analysis = o?.analysis ?? null;
  const base = analysis ? `/analyses/${encodeURIComponent(analysis)}` : null;
  const display = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;

  const error = (uncertain ? null : visitError) ?? (overviewRead ? (overview.error ?? items.error) : null);
  const failed = error ? errorText(error) : null;
  const body = (count: (id: string) => number) => {
    if (!o || !items.data)
      return failed ? (
        <StatusMessage kind="error">{failed}</StatusMessage>
      ) : (
        <StatusMessage kind="loading">Reading the comments…</StatusMessage>
      );
    const context: CommentsContext = {
      pr,
      overview: o,
      items: items.data,
      needsYou,
      statusHost,
      focusQueueStatus,
      askAnswers: (item) => Math.max(item.ask_answers, count(item.id)),
      login,
      now,
    };
    // A failed later read keeps the page (and what was typed on it) with the last data, and says so.
    return (
      <>
        {failed && (
          <div className="mb-3">
            <StatusMessage kind="error">
              The comments could not be read again: {failed} The page shows the last read; it is read again
              shortly.
            </StatusMessage>
          </div>
        )}
        <Outlet context={context} />
      </>
    );
  };

  return (
    <div className="flex min-h-screen flex-col">
      <ReviewTopBar
        pr={{
          repo: pr.repo,
          number: pr.number,
          title: o?.title ?? "",
          html_url: o?.url ?? prGithubUrl(pr),
          head_sha: o?.head ?? "",
        }}
        login={me.data?.user?.login ?? null}
        view={
          o
            ? {
                current: "comments",
                flowsHref: base,
                filesHref: base && `${base}/files`,
                depsHref: base && `${base}/deps`,
                commentsHref: hrefs.questions,
                prPageHref: prPath(pr),
                onNavigate: go,
              }
            : undefined
        }
      />
      <main id="main" className="w-full px-3 py-3.5 sm:px-5">
        {!o && <h1 className="sr-only">Comments of {display}</h1>}
        {(uncertain || retryingVisit) && (
          <div className="mb-3">
            <Banner
              tone="warn"
              title="Could not confirm this Comments visit."
              actions={
                (visitUnavailable(visitError) || retryingVisit) && (
                  <Tooltip
                    tip={busyTip(
                      "Records this visit again so diffpilot can count new GitHub imports.",
                      retryingVisit,
                    )}
                  >
                    <button
                      type="button"
                      className={buttonClass("plain", true)}
                      aria-disabled={retryingVisit || undefined}
                      onClick={() => !retryingVisit && visit()}
                    >
                      {retryingVisit ? "Retrying…" : "Retry"}
                    </button>
                  </Tooltip>
                )
              }
            >
              New imports use the visit recorded by the server.
            </Banner>
          </div>
        )}
        {o?.read.failure && (
          <div className="mb-3">
            <Banner
              tone="error"
              title={`GitHub could not be read (${READ_FAILURE[o.read.failure] ?? o.read.failure}).`}
            >
              {o.read.read_at
                ? `Showing the read of ${clockText(o.read.read_at, now)}`
                : "Nothing was read yet"}
              {o.read.next_read_at ? `; next read at ${clockText(o.read.next_read_at, now)}` : ""}. Decisions
              on these items still work.
            </Banner>
          </div>
        )}
        {o && (
          <div className="mb-3 flex flex-wrap items-center gap-3 rounded-lg border border-border bg-surface px-3 py-2">
            <h1 className="m-0 min-w-0 text-sm font-semibold wrap-anywhere">
              {display} <span className="font-normal text-muted">{o.title}</span>{" "}
              <OpenExternal href={o.url} label={`Open ${display} on GitHub`} />
            </h1>
            <StatusPopover
              badge={o.badge}
              rounds={o.rounds.length}
              notes={page.data?.plugin_notes}
              ci={page.data?.ci}
              read={o.read}
              now={now}
              itemId={display}
              busy={read.isPending || items.isLoading}
              onRead={() => read.mutate()}
            />
            <div className="ml-auto">
              <WatchControl
                variant="state"
                watch={o.watch ?? null}
                now={now}
                busy={watch.isPending}
                onChange={(on) => watch.mutate(on)}
              />
            </div>
          </div>
        )}
        {read.isError && <StatusMessage kind="error">{errorText(read.error)}</StatusMessage>}
        {assessmentIssue?.code === "credential_missing" && (
          <StatusMessage kind="empty">
            <Link className="text-accent underline" to={ACCOUNT_SETTINGS_PATH}>
              Link a Codex account in Settings
            </Link>{" "}
            to assess comments.
          </StatusMessage>
        )}
        {assessmentIssue?.code === "assessment_daily_limit" && (
          <StatusMessage kind="empty">
            Daily comment assessment limit reached.
            {assessmentIssue.resets_at && (
              <>
                {" "}
                The limit resets at{" "}
                <time dateTime={assessmentIssue.resets_at}>{dateTimeText(assessmentIssue.resets_at)}</time>.
              </>
            )}
          </StatusMessage>
        )}
        {assessmentIssue?.code === "busy" && (
          <StatusMessage kind="empty">
            A pass is still running. New comments can be assessed on your next visit.
          </StatusMessage>
        )}
        {assessmentIssue?.code === "failed" && (
          <StatusMessage kind="error">
            Could not start comment assessment. {assessmentIssue.message}
          </StatusMessage>
        )}
        {watch.isError && <StatusMessage kind="error">{errorText(watch.error)}</StatusMessage>}
        {o && (
          <>
            {assessmentSettings.data && (
              <AssessmentControls
                settings={assessmentSettings.data}
                now={now}
                prKey={`${pr.repo.owner}/${pr.repo.name}#${pr.number}`}
                busy={saveAssessment.isPending}
                error={saveAssessment.isError ? errorText(saveAssessment.error) : undefined}
                onSave={(config) =>
                  saveAssessment.mutate({ config, revision: assessmentSettings.data!.revision })
                }
              />
            )}
            {assessmentSettings.isError && (
              <StatusMessage kind="error">{errorText(assessmentSettings.error)}</StatusMessage>
            )}
            <AssessmentComparisonPanel
              expanded={showComparison}
              report={comparison.data}
              loading={comparison.isPending}
              error={comparison.isError ? errorText(comparison.error) : undefined}
              onOpenChange={setShowComparison}
            />
          </>
        )}
        {o && (
          <CommentsTabs
            current={current}
            hrefs={hrefs}
            questions={needsYou}
            onNavigate={go}
            activeTabRef={activeTab}
          />
        )}
        {o && (
          <AssessmentBanner
            progress={o.assessment}
            head={o.head}
            prKey={display}
            now={now}
            onHide={() => {
              if (current === "questions") {
                focusStatusOnMount.current = true;
                focusQueueStatus();
              } else {
                activeTab.current?.focus();
              }
            }}
          />
        )}
        {o && (unseen > 0 || showImports) && (
          <ImportedDecisions
            count={unseen}
            items={showImports ? imports.data : undefined}
            error={showImports && imports.isError ? errorText(imports.error) : undefined}
            expanded={showImports}
            href={`${pathname}${importSearch.size ? `?${importSearch}` : ""}`}
            onNavigate={go}
          />
        )}
        <AnalysisPathLinks analysisId={analysis} head={o?.head ?? null}>
          <WithAsk pr={pr} items={items.data}>
            {body}
          </WithAsk>
        </AnalysisPathLinks>
      </main>
    </div>
  );
}
