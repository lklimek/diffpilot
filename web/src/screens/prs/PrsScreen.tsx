import { useAssessmentSettings } from "../../api/assessment-settings";
import { AssessmentQuotaText } from "../../components/comments/AssessmentQuotaText";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router";
import { api, errorText } from "../../api/client";
import { useMe, useMyAnalyses } from "../../api/queries";
import { useWatches } from "../../api/review-loop";
import type { PrRef } from "../../api/generated/types";
import { useNow } from "../../app/now";
import { Banner } from "../../components/banner/Banner";
import { buttonClass } from "../../components/button/button";
import { PrHistory } from "../../components/pr-history/PrHistory";
import { PrUrlForm } from "../../components/pr-form/PrUrlForm";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { ALL_CARRY_OVERS_PATH, parsePrRef, prPath } from "../pr/paths";
import { MY_WORK_PATH, PRS_PATH } from "../shell/paths";
import { historyFromLegacy, historyFromServer, historyKey } from "./history";

const PR_HISTORY_PAGE_SIZE = 50;

const FILTERS = [
  { value: "all", name: "All" },
  { value: "watched", name: "Watched" },
  { value: "analysed", name: "Analysed" },
] as const;

const LOGIN_ERRORS = new Map<string, [string, string]>([
  [
    "login_not_allowed",
    [
      "This GitHub login is not allowed.",
      "Only the GitHub logins this diffpilot instance allows can sign in. Ask its owner to add yours.",
    ],
  ],
  [
    "oauth_failed",
    ["Sign-in with GitHub did not complete.", "GitHub did not confirm the sign-in. Try again."],
  ],
  [
    "state_mismatch",
    ["The sign-in expired.", "It took too long or was started in another tab. Sign in again."],
  ],
]);

export function LegacyReviewRedirect() {
  const [search] = useSearchParams();
  const params = new URLSearchParams(search);
  const value = params.get("pr");
  const pr = value === null ? null : parsePrRef(value);
  if (pr) params.delete("pr");
  return <Navigate to={{ pathname: pr ? prPath(pr) : PRS_PATH, search: params.toString() }} replace />;
}

export function LegacyWatchingRedirect() {
  const [params] = useSearchParams();
  const search = new URLSearchParams(params);
  search.set("show", "watched");
  return <Navigate to={{ pathname: PRS_PATH, search: search.toString() }} replace />;
}

export function PrsScreen() {
  const me = useMe();
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const initialUrl = params.get("pr");
  const loginCode = params.get("login_error");
  const loginError =
    loginCode === null ? null : (LOGIN_ERRORS.get(loginCode) ?? ["Sign-in failed.", "Try again."]);
  const go = (href: string) => void navigate(href);

  return (
    <div className="grid max-w-[1100px] min-w-0 gap-4">
      <h1 className="text-xl font-bold">Pull requests</h1>
      {loginError && (
        <Banner tone="error" title={loginError[0]}>
          {loginError[1]}
        </Banner>
      )}
      {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
      {me.isError && <StatusMessage kind="error">{errorText(me.error)}</StatusMessage>}
      {me.isSuccess && !me.data.user && <SignInCard loginHref="/auth/github/login" />}
      {me.data?.user && (
        <>
          <PrUrlForm
            key={initialUrl}
            initialUrl={initialUrl ?? undefined}
            initialError={initialUrl !== null && !parsePrRef(initialUrl)}
            onOpen={(value) => {
              const pr = parsePrRef(value);
              if (!pr) return false;
              const next = new URLSearchParams(params);
              for (const key of ["pr", "show", "q", "limit", "login_error"]) next.delete(key);
              void navigate({ pathname: prPath(pr), search: next.toString() });
              return true;
            }}
          />
          <History params={params} setParams={setParams} go={go} />
        </>
      )}
    </div>
  );
}

function History({
  params,
  setParams,
  go,
}: {
  params: URLSearchParams;
  setParams: ReturnType<typeof useSearchParams>[1];
  go: (href: string) => void;
}) {
  const client = useQueryClient();
  const watches = useWatches();
  const quota = useAssessmentSettings().data?.quota;
  const history = useInfiniteQuery({
    queryKey: ["pr-history"],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      api.listMyPrs({ limit: PR_HISTORY_PAGE_SIZE, cursor: pageParam ?? undefined }, signal),
    getNextPageParam: (page) => page?.next_cursor ?? undefined,
    retry: false,
  });
  const fallback = history.data?.pages[0] === null;
  const analyses = useMyAnalyses(fallback, 100);
  const watch = useMutation({
    mutationFn: ({ pr, enabled }: { pr: PrRef; enabled: boolean }) =>
      enabled ? api.watchPr(pr) : api.unwatchPr(pr),
    onSuccess: (_data, { pr }) =>
      Promise.all([
        client.invalidateQueries({ queryKey: ["watches"] }),
        client.invalidateQueries({ queryKey: ["pr-history"] }),
        client.invalidateQueries({ queryKey: ["pr-page", pr.repo.owner, pr.repo.name, pr.number] }),
        client.invalidateQueries({ queryKey: ["dashboard-items"] }),
      ]),
  });
  const now = new Date(useNow());
  const entries = fallback
    ? historyFromLegacy(analyses.data?.analyses ?? [], watches.data?.watches ?? [])
    : [
        ...new Map(
          history.data?.pages.flatMap(
            (page) => page?.prs.map((row) => [historyKey(row.pr), historyFromServer(row)] as const) ?? [],
          ),
        ).values(),
      ];
  const show = params.get("show") ?? "all";
  const filter = params.get("q") ?? "";
  const query = filter.trim().toLowerCase();
  const count = Number(params.get("limit"));
  const limit = Number.isSafeInteger(count) && count > PR_HISTORY_PAGE_SIZE ? count : PR_HISTORY_PAGE_SIZE;
  const filtered = entries.filter((entry) => {
    if (show === "watched" && !entry.watch) return false;
    if (show === "analysed" && !entry.analysis) return false;
    return (
      !query || `${entry.pr.repo.owner}/${entry.pr.repo.name} ${entry.title}`.toLowerCase().includes(query)
    );
  });
  const setFilter = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    next.delete("limit");
    setParams(next, { replace: key === "q", preventScrollReset: true });
  };
  const pending = history.isPending || (fallback && (watches.isPending || analyses.isPending));
  const rows = (fallback ? filtered.slice(0, limit) : filtered).map(
    ({ pr, title, opened_at, last_activity_at, analysis, watch }) => ({
      id: `${pr.repo.owner}/${pr.repo.name}#${pr.number}`,
      pr,
      title,
      prHref: prPath(pr),
      reviewHref:
        analysis?.status === "finished" ? `/analyses/${encodeURIComponent(analysis.id)}` : undefined,
      lastActivityAt: last_activity_at,
      openedAt: opened_at,
      requestedAt: analysis?.requestedAt,
      analysisStatus: analysis?.status,
      headMoved: analysis?.headMoved,
      watch,
    }),
  );
  const announcedWatch =
    watch.isSuccess && watch.variables
      ? `${watch.variables.enabled ? "Watching" : "Stopped watching"} ${historyKey(watch.variables.pr)}.`
      : null;
  return (
    <section aria-labelledby="history-heading" className="grid min-w-0 gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h2 id="history-heading" className="mr-auto text-base font-semibold">
          Your pull requests
        </h2>
        <div role="group" aria-label="Show" className="flex flex-wrap gap-1">
          {FILTERS.map(({ value, name }) => {
            const active = show === value || (value === "all" && show !== "watched" && show !== "analysed");
            return (
              <Tooltip key={value} tip={`Show ${name.toLowerCase()} pull requests`}>
                <button
                  type="button"
                  aria-pressed={active}
                  className={buttonClass(active ? "primary" : "plain", true)}
                  onClick={() => setFilter("show", value === "all" ? "" : value)}
                >
                  {name}
                </button>
              </Tooltip>
            );
          })}
        </div>
        <input
          type="search"
          aria-label="Filter by repository or title"
          value={filter}
          onChange={(event) => setFilter("q", event.target.value)}
          placeholder="Filter by repository or title"
          className="min-h-8 w-full min-w-0 rounded border border-muted bg-surface px-2 py-1 text-sm sm:w-56"
        />
      </div>
      <div className="flex flex-wrap gap-x-2 text-xs text-muted">
        {watches.data && (
          <span>
            Watching {watches.data.watches.filter(({ state }) => state !== "ended").length} of{" "}
            {watches.data.limit}
          </span>
        )}
        {quota && (
          <span>
            <AssessmentQuotaText quota={quota} />
          </span>
        )}
        <Link to={ALL_CARRY_OVERS_PATH}>Carry-overs on all pull requests</Link>
      </div>
      {history.isError && <StatusMessage kind="error">{errorText(history.error)}</StatusMessage>}
      {watches.isError && <StatusMessage kind="error">{errorText(watches.error)}</StatusMessage>}
      {analyses.isError && <StatusMessage kind="error">{errorText(analyses.error)}</StatusMessage>}
      {watch.isError && <StatusMessage kind="error">{errorText(watch.error)}</StatusMessage>}
      <div role="status" className="sr-only">
        {pending
          ? "Loading pull requests…"
          : `${rows.length} ${rows.length === 1 ? "pull request" : "pull requests"} shown.`}
        {announcedWatch && ` ${announcedWatch}`}
      </div>
      {pending && <p className="text-sm text-muted">Loading pull requests…</p>}
      {rows.length > 0 && (
        <PrHistory
          rows={rows}
          now={now}
          busy={watch.isPending}
          canWatch={Boolean(watches.data)}
          onWatch={(pr, enabled) => watch.mutate({ pr, enabled })}
          onNavigate={go}
        />
      )}
      {!pending && !history.isError && rows.length === 0 && (
        <p className="rounded-lg border border-border bg-surface p-3 text-sm text-muted">
          {entries.length ? (
            "No pull requests match these filters."
          ) : (
            <>
              No pull requests yet. Open one by URL, or from <Link to={MY_WORK_PATH}>My work</Link>.
            </>
          )}
        </p>
      )}
      {(history.hasNextPage || (fallback && filtered.length > limit)) && (
        <Tooltip tip="Shows more pull requests from your history.">
          <button
            type="button"
            className={`${buttonClass("plain")} w-fit`}
            aria-disabled={history.isFetchingNextPage || undefined}
            onClick={() => {
              if (history.isFetchingNextPage) return;
              if (!fallback) void history.fetchNextPage();
              else {
                const next = new URLSearchParams(params);
                next.set("limit", String(limit + PR_HISTORY_PAGE_SIZE));
                setParams(next, { preventScrollReset: true });
              }
            }}
          >
            {history.isFetchingNextPage ? "Loading…" : "Show more"}
          </button>
        </Tooltip>
      )}
    </section>
  );
}
