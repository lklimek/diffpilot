import { useCallback, useMemo, type ReactNode } from "react";
import { Outlet, useLocation, useNavigate, useOutletContext, useParams } from "react-router";
import { ApiError } from "../../api/client";
import type { AnalysisId, CommentItem, CommentsOverview, PrPage, PrRef } from "../../api/generated/types";
import { useMe, useQuestions } from "../../api/queries";
import { useCommentItems, useCommentsOverview, usePrPage, useReadComments } from "../../api/review-loop";
import { useNow } from "../../app/now";
import { usePrTabSubject } from "../../app/tabStatus";
import { useUserEventNotifications } from "../../app/userEvents";
import { Banner } from "../../components/banner/Banner";
import { buttonClass } from "../../components/button/button";
import { CommentsTabs, type CommentsPage } from "../../components/comments/CommentsTabs";
import { autoPassText, PrStatusStrip } from "../../components/comments/PrHeader";
import { clockText } from "../../components/format/time";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { TopBar } from "../../components/top-bar/TopBar";
import { AskChatProvider } from "../review/askChat";
import { commentsPath, prOfParams, prPath } from "../pr/paths";

/** What every Comments page gets from the layout. */
export interface CommentsContext {
  pr: PrRef;
  overview: CommentsOverview;
  items: CommentItem[];
  page: PrPage | undefined;
  /** Ask answers about an item (stored count, or the live chat's, whichever is higher). */
  askAnswers: (item: CommentItem) => number;
  // TODO(WP13-A CCR): plan RC7 Ask without an analysis has no route yet; Ask stays disabled with a tip.
  /** Ask is available (the PR has an analysis). */
  askAvailable: boolean;
  login: string;
  now: Date;
}

/** The layout's data for a Comments page. */
export const useComments = () => useOutletContext<CommentsContext>();

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

const READ_FAILURE = {
  rate_limited: "rate limit",
  unavailable: "GitHub did not answer as expected",
  sign_in_required: "sign in again",
} as const;

/** Counts the live chat's answers per comment item (with an analysis only). */
function WithAsk({
  analysis,
  children,
}: {
  analysis: AnalysisId;
  children: (count: (id: string) => number) => ReactNode;
}) {
  const questions = useQuestions(analysis);
  const count = useCallback(
    (id: string) =>
      (questions.data ?? []).filter((q) => {
        const anchor = q.anchor as { kind: string; item?: string };
        return q.state === "answered" && anchor.kind === "comment_item" && anchor.item === id;
      }).length,
    [questions.data],
  );
  return <AskChatProvider analysisId={analysis}>{children(count)}</AskChatProvider>;
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
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const me = useMe();
  const login = me.data?.user?.login ?? "anonymous";
  const overview = useCommentsOverview(pr);
  const items = useCommentItems(pr);
  const page = usePrPage(pr);
  const read = useReadComments(pr);
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
  const o = overview.data;
  const analysis = o?.analysis ?? null;
  const base = analysis ? `/analyses/${encodeURIComponent(analysis)}` : null;
  const display = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;

  const body = (count: (id: string) => number) => {
    if (overview.isError || items.isError)
      return <StatusMessage kind="error">{errorText(overview.error ?? items.error)}</StatusMessage>;
    if (!o || !items.data) return <StatusMessage kind="loading">Reading the comments…</StatusMessage>;
    const context: CommentsContext = {
      pr,
      overview: o,
      items: items.data,
      page: page.data,
      askAnswers: (item) => Math.max(item.ask_answers, count(item.id)),
      askAvailable: analysis !== null,
      login,
      now,
    };
    return <Outlet context={context} />;
  };

  return (
    <div className="flex min-h-screen flex-col">
      <TopBar
        pr={
          o
            ? { repo: pr.repo, number: pr.number, title: o.title, html_url: o.url, head_sha: o.head }
            : undefined
        }
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
                prContextHref: prPath(pr),
                onNavigate: go,
              }
            : undefined
        }
      />
      <main id="main" className="w-full px-5 py-3.5">
        <h1 className="sr-only">Comments of {display}</h1>
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
          <div className="mb-3 flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1">
              <PrStatusStrip
                badge={o.badge}
                rounds={o.rounds.length}
                notes={page.data?.plugin_notes ?? []}
                now={now}
                itemId={display}
              />
            </div>
            {page.data?.watch && (
              <span className="rounded-full border border-low-bd bg-low-bg px-2 text-xs font-semibold text-low">
                Watching{page.data.watch.last_auto_pass ? ` · ${autoPassText(page.data.watch, now)}` : ""}
              </span>
            )}
            <span className="text-xs text-muted">
              {o.read.read_at ? `GitHub read ${clockText(o.read.read_at, now)}` : "GitHub not read yet"}
            </span>
            <Tooltip tip="Read the threads, reviews and comments from GitHub again.">
              <button
                type="button"
                aria-disabled={read.isPending || undefined}
                onClick={() => !read.isPending && read.mutate()}
                className={buttonClass("plain", true)}
              >
                Read GitHub again
              </button>
            </Tooltip>
          </div>
        )}
        {read.isError && <StatusMessage kind="error">{errorText(read.error)}</StatusMessage>}
        {o && <CommentsTabs current={current} hrefs={hrefs} questions={o.counts.undecided} onNavigate={go} />}
        {analysis ? <WithAsk analysis={analysis}>{body}</WithAsk> : body(() => 0)}
      </main>
    </div>
  );
}
