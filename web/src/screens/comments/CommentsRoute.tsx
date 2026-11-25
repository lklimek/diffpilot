import {
  useAssessmentSettings,
  useSetAssessmentSettings,
  useAssessmentComparison,
} from "../../api/assessment-settings";
import { AssessmentControls } from "../../components/comments/AssessmentControls";
import { AssessmentComparisonPanel } from "../../components/comments/AssessmentComparisonReport";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Outlet, useLocation, useNavigate, useOutletContext, useParams } from "react-router";
import { ApiError } from "../../api/client";
import type { CommentItem, PrPage, PrRef } from "../../api/generated/types";
import type { ReviewComment, ReviewCommentsOverview } from "../../api/provisional/comments";
import { useMe } from "../../api/queries";
import {
  useCommentItems,
  useCommentsOverview,
  usePrPage,
  usePrQuestions,
  useReadComments,
  useVisitComments,
  useWatchPr,
} from "../../api/review-loop";
import { useNow } from "../../app/now";
import { usePrTabSubject } from "../../app/tabStatus";
import { useUserEventNotifications } from "../../app/userEvents";
import { Banner } from "../../components/banner/Banner";
import { AssessmentBanner } from "../../components/comments/AssessmentBanner";
import { OpenExternal } from "../../components/dashboard/common/OpenExternal";
import { WatchControl } from "../../components/watch/WatchControl";
import { CommentsTabs, type CommentsPage } from "../../components/comments/CommentsTabs";
import { commentsByNo, needsYouCount } from "../../components/comments/queueStatus";
import { READ_FAILURE } from "../../components/comments/parts";
import { StatusPopover } from "../../components/status/StatusPopover";
import { clockText } from "../../components/format/time";
import { StatusMessage } from "../../components/status/StatusMessage";
import { prGithubUrl } from "../../components/github/links";
import { ReviewTopBar } from "../shell/ReviewTopBar";
import { AnalysisPathLinks } from "../review/AnalysisPathLinks";
import { PrAskChatProvider } from "../review/askChat";
import { commentsPath, prOfParams, prPath } from "../pr/paths";

/** What every Comments page gets from the layout. */
export interface CommentsContext {
  pr: PrRef;
  overview: ReviewCommentsOverview;
  items: ReviewComment[];
  needsYou: number;
  statusHost: RefObject<HTMLDivElement | null>;
  focusQueueStatus: () => void;
  page: PrPage | undefined;
  /** Ask answers about an item (stored count, or the live chat's, whichever is higher). */
  askAnswers: (item: CommentItem) => number;
  login: string;
  now: Date;
}

/** The layout's data for a Comments page. */
export const useComments = () => useOutletContext<CommentsContext>();

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

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

function CommentsVisit({ pr }: { pr: PrRef }) {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const me = useMe();
  const login = me.data?.user?.login ?? "anonymous";
  const { mutate: recordVisit, isSuccess: visited, error: visitError } = useVisitComments(pr);
  const entered = useRef(false);
  useEffect(() => {
    if (entered.current) return;
    entered.current = true;
    recordVisit();
  }, [recordVisit]);
  const overview = useCommentsOverview(pr, visited);
  // Load items after the initial overview identifies the stored PR.
  const overviewRead = visited && overview.isFetchedAfterMount;
  const items = useCommentItems(pr, overviewRead);
  const page = usePrPage(pr);
  const read = useReadComments(pr);
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
  useUserEventNotifications();
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
  const needsYou = needsYouCount(o?.counts.undecided ?? 0, o?.queue, commentsByNo(items.data));
  const watching = o?.watch === undefined ? page.data?.watch : o.watch;
  const analysis = o?.analysis ?? null;
  const base = analysis ? `/analyses/${encodeURIComponent(analysis)}` : null;
  const display = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;

  const error = visitError ?? (overviewRead ? (overview.error ?? items.error) : null);
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
      page: page.data,
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
              notes={page.data?.plugin_notes ?? []}
              read={o.read}
              ci={page.data?.ci}
              now={now}
              itemId={display}
              busy={read.isPending}
              onRead={() => read.mutate()}
            />
            {watching !== undefined && (
              <div className="ml-auto">
                <WatchControl
                  variant="state"
                  watch={watching}
                  now={now}
                  busy={watch.isPending}
                  onChange={(on) => watch.mutate(on)}
                />
              </div>
            )}
          </div>
        )}
        {read.isError && <StatusMessage kind="error">{errorText(read.error)}</StatusMessage>}
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
        <AnalysisPathLinks analysisId={analysis} head={o?.head ?? null}>
          <WithAsk pr={pr} items={items.data}>
            {body}
          </WithAsk>
        </AnalysisPathLinks>
      </main>
    </div>
  );
}
