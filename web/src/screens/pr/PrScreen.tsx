import { Tooltip } from "../../components/tooltip/Tooltip";
import { Link, useNavigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import { useMe } from "../../api/queries";
import { usePrPage, useWatchPr } from "../../api/review-loop";
import { useNow } from "../../app/now";
import { useUserEventNotifications } from "../../app/userEvents";
import { Banner } from "../../components/banner/Banner";
import { buttonClass } from "../../components/button/button";
import { CollapsibleSection } from "../../components/collapsible/CollapsibleSection";
import { autoPassText, PrStatusStrip, WatchToggle } from "../../components/comments/PrHeader";
import { ProcessCommentsCard } from "../../components/comments/ProcessCommentsCard";
import { OpenExternal } from "../../components/dashboard/common/OpenExternal";
import { clockText } from "../../components/format/time";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";
import { MY_WORK_PATH, REVIEW_START_PATH } from "../shell/paths";
import { LiveReview } from "./LiveReview";
import { commentsPath, prOfParams } from "./paths";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** V0 PR page (plan §3.1): header, Watch, "Review a PR" (the analysis's progress) and "Process PR comments". */
export function PrScreen() {
  const pr = prOfParams(useParams());
  const page = usePrPage(pr);
  const me = useMe();
  const login = me.data?.user?.login ?? null;
  const watch = useWatchPr(pr);
  const navigate = useNavigate();
  const now = new Date(useNow());
  useUserEventNotifications();
  const data = page.data;
  const display = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;
  const key = (section: string) => `${login ?? "anonymous"}:pr-page:${section}`;
  return (
    <div className="flex min-h-screen flex-col">
      <TopBar
        pr={
          data
            ? { repo: pr.repo, number: pr.number, title: data.title, html_url: data.url, head_sha: data.head }
            : undefined
        }
        ci={data?.ci ?? undefined}
        checksHref={data ? `${data.url}/checks` : undefined}
        login={login}
      />
      <main id="main" className="mx-auto grid w-full max-w-[1600px] gap-3 px-5 py-4">
        {page.isPending && <StatusMessage kind="loading">Loading the pull request…</StatusMessage>}
        {page.isError && (
          <>
            <StatusMessage kind="error">{errorText(page.error)}</StatusMessage>
            <Link to={MY_WORK_PATH}>My work</Link>
          </>
        )}
        {data && (
          <>
            <div className="flex flex-wrap items-start gap-3">
              <div className="min-w-0 flex-1">
                <h1 className="m-0 text-xl font-bold">
                  {display} <span className="font-normal text-muted">{data.title}</span>{" "}
                  <OpenExternal href={data.url} label={`Open ${display} on GitHub`} />
                </h1>
                <p className="m-0 text-xs text-muted">
                  by {data.author}
                  {data.base && (
                    <>
                      {" "}
                      · base <code>{data.base}</code>
                    </>
                  )}
                  {data.stack_parent && ` (stacked on #${data.stack_parent.number})`}
                  {(data.draft || data.state) && ` · ${data.draft ? "draft" : data.state.toLowerCase()}`} ·
                  head <code>{data.head.slice(0, 7)}</code>
                </p>
              </div>
              <WatchToggle
                watch={data.watch}
                busy={watch.isPending}
                onChange={(on) => watch.mutate(on)}
                now={now}
              />
            </div>
            {watch.isError && <StatusMessage kind="error">{errorText(watch.error)}</StatusMessage>}
            {data.watch?.last_auto_pass && (
              <Banner tone="info" title="Watching.">
                {autoPassText(data.watch, now)} at {clockText(data.watch.last_auto_pass.at, now)}; you were
                notified where notifications are on.
              </Banner>
            )}
            <PrStatusStrip
              badge={data.badge}
              rounds={data.comments.rounds}
              notes={data.plugin_notes}
              now={now}
              itemId={display}
            />
            <div className="grid items-start gap-3.5 lg:grid-cols-2">
              <div className="rounded-lg border border-border bg-surface px-4 pb-3.5">
                <CollapsibleSection title="Review a PR" level={2} storageKey={key("review")}>
                  {data.analysis ? (
                    <LiveReview analysisId={data.analysis.id} />
                  ) : (
                    <>
                      <p>No analysis yet.</p>
                      <Tooltip tip="Opens Review a PR with this pull request: check it and start its analysis.">
                        <a
                          href={`${REVIEW_START_PATH}?pr=${encodeURIComponent(data.url)}`}
                          onClick={(event) => {
                            event.preventDefault();
                            void navigate(`${REVIEW_START_PATH}?pr=${encodeURIComponent(data.url)}`);
                          }}
                          className={buttonClass("primary")}
                        >
                          Check and start
                        </a>
                      </Tooltip>
                    </>
                  )}
                </CollapsibleSection>
              </div>
              <ProcessCommentsCard
                summary={data.comments}
                now={now}
                questionsHref={commentsPath(pr)}
                tableHref={commentsPath(pr, "table")}
                onNavigate={(href) => void navigate(href)}
                storageKey={key("comments")}
              />
            </div>
          </>
        )}
      </main>
    </div>
  );
}
