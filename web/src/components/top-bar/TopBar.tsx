import type { MouseEvent, ReactNode } from "react";
import { inApp } from "../inApp";
import { GithubMark } from "../github/GithubMark";
import { CHECKS_LINK_TIP, ciBadgeText, ciText, ciTip } from "../section/ci";
import type { CiStatus, LlmLanguage, PrSummary } from "../../api/generated/types";
import { LanguageTag } from "../language-tag/LanguageTag";
import { PENDING_INCOMPLETE } from "../section/format";
import { MenuButton, MenuItem } from "../menu/MenuButton";
import { Tooltip } from "../tooltip/Tooltip";

interface TopBarProps {
  hideContextChanges?: boolean;
  onHideContextChanges?: (hide: boolean) => void;
  /** The pull request under review; absent outside the review. */
  pr?: TopBarPr;
  currentPath?: string;
  /** CI status of the head commit. */
  ci?: CiStatus;
  /** The PR's checks page on GitHub; the CI summary links to it (new tab) when given. */
  checksHref?: string;
  /** Language of the analysis's LLM texts; no tag when absent. */
  language?: LlmLanguage;
  /** Account menu of the signed-in reader. */
  userMenu?: ReactNode;
  /** Review progress: flows with every step ticked, out of all flows. */
  progress?: { reviewed: number; total: number };
  /** The progress counter as the flows dropdown (both review views); replaces the plain `progress` text. */
  progressMenu?: ReactNode;
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no item when omitted. */
  pendingCount?: number | `${number}+` | "unavailable" | "loading";
  /** Opens the pending-review list. */
  onOpenPending?: () => void;
  /** The review views of the analysis: with it the bar starts with the breadcrumb and the view menu. */
  view?: ReviewViews;
  /** Pull requests index in the breadcrumb. */
  prsHref?: string;
  onNavigate?: (href: string) => void;
}

/** What the bar shows of a pull request. */
export type TopBarPr = Pick<PrSummary, "repo" | "number" | "title" | "html_url" | "head_sha">;

/** The views of one analysis and where the breadcrumb leads (in-app links). */
export interface ReviewViews {
  current: "flows" | "files" | "deps" | "comments";
  /** Null without an analysis: the entry reads "Needs an analysis" and leads to `prPageHref`. */
  flowsHref: string | null;
  filesHref: string | null;
  depsHref: string | null;
  /** Comments (works without an analysis); no entry when absent. */
  commentsHref?: string;
  /** The PR page, where an analysis starts. */
  prPageHref: string;
  /** Follows an in-app link (a plain click; other clicks keep the browser's behaviour). */
  onNavigate: (href: string) => void;
}

const VIEW_NAMES = { flows: "Flows", files: "Files", deps: "Dependencies", comments: "Comments" } as const;

const SEP = (
  <span aria-hidden className="text-muted">
    ›
  </span>
);

function Breadcrumb({
  pr,
  view,
  prsHref,
  onNavigate,
}: {
  pr: TopBarPr;
  view?: ReviewViews;
  prsHref: string;
  onNavigate?: (href: string) => void;
}) {
  const full = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;
  const follow = (event: MouseEvent, href: string) => {
    if (onNavigate) inApp(event, () => onNavigate(href));
  };
  const label = (
    <>
      <span aria-hidden className="review-repo hidden min-w-0 sm:inline-flex">
        <span className="review-owner hidden min-w-0 truncate min-[1440px]:inline">
          <bdi>{pr.repo.owner}/</bdi>
        </span>
        <span className="review-repo-name min-w-0 truncate">{pr.repo.name}</span>
      </span>{" "}
      <span aria-hidden className="shrink-0">
        #{pr.number}
      </span>
      <span className="sr-only">{full}</span>
    </>
  );
  const prClass = "flex min-h-6 min-w-0 items-baseline gap-1 font-semibold text-text";
  return (
    <nav
      aria-label="Breadcrumb"
      className="review-crumb flex min-w-max shrink-0 items-center gap-1.5 sm:min-w-0 sm:shrink"
    >
      <span className="review-crumb-lead hidden shrink-0 items-center gap-1.5 sm:inline-flex">
        <a
          href="/"
          onClick={(event) => follow(event, "/")}
          className="review-brand font-bold text-text no-underline"
        >
          diff<span className="text-accent">pilot</span>
        </a>
        {SEP}
        <a
          href={prsHref}
          onClick={(event) => follow(event, prsHref)}
          className="font-semibold text-text no-underline hover:underline"
        >
          Pull requests
        </a>
        {SEP}
      </span>
      <Tooltip tip={full} className="min-w-0">
        {view ? (
          <a
            href={view.prPageHref}
            onClick={(event) => inApp(event, () => view.onNavigate(view.prPageHref))}
            className={`${prClass} no-underline hover:underline`}
          >
            {label}
          </a>
        ) : (
          <span aria-current="page" tabIndex={0} className={prClass}>
            {label}
          </span>
        )}
      </Tooltip>
      <Tooltip tip="Open the pull request on GitHub (new tab)">
        <a
          href={pr.html_url}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={`Open ${pr.repo.owner}/${pr.repo.name}#${pr.number} on GitHub, new tab`}
          className="inline-flex size-6 shrink-0 items-center justify-center rounded border border-border"
        >
          <GithubMark />
        </a>
      </Tooltip>
      {view && (
        <>
          {SEP}
          <ViewMenu view={view} />
        </>
      )}
    </nav>
  );
}

function ViewMenu({ view }: { view: ReviewViews }) {
  const href = {
    flows: view.flowsHref,
    files: view.filesHref,
    deps: view.depsHref,
    comments: view.commentsHref,
  };
  return (
    <div className="shrink-0">
      <MenuButton
        label={`View: ${VIEW_NAMES[view.current]}`}
        trigger={
          <>
            {VIEW_NAMES[view.current]} <span aria-hidden="true">▾</span>
          </>
        }
        tip={`Switch between the views of this review: Flows (step by step), Files (one file at a time; s switches between these two) and Dependencies (crates this pull request changes, what they depend on in your organisations, and what was added for other crates' types).${view.commentsHref ? " Comments: decisions on the review comments of this pull request; needs no analysis." : ""}`}
        className={`${BUTTON} border-accent`}
      >
        {(["flows", "files", "deps", "comments"] as const)
          .filter((v) => href[v] !== undefined)
          .map((v) => (
            <MenuItem
              key={v}
              label={VIEW_NAMES[v]}
              selection={{ kind: "radio", checked: v === view.current }}
              description={href[v] === null ? "Needs an analysis" : undefined}
              href={href[v] ?? view.prPageHref}
              onNavigate={(target) => {
                if (v !== view.current) view.onNavigate(target);
              }}
            />
          ))}
      </MenuButton>
    </div>
  );
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-[12.5px] font-semibold text-text hover:border-accent";

function pendingText(count: NonNullable<TopBarProps["pendingCount"]>): string {
  if (count === "unavailable") return "not available";
  if (count === "loading") return "…";
  return String(count);
}

/**
 * Top bar (brief §4 rule 5): brand (in the review views the breadcrumb with the view menu), pull request, head SHA,
 * CI status, "Your progress k / n", pending-review count and user menu.
 */
export function TopBar({
  hideContextChanges = true,
  onHideContextChanges,
  pr,
  currentPath,
  ci,
  checksHref,
  language,
  userMenu,
  progress,
  pendingCount,
  onOpenPending,
  view,
  prsHref = "/prs",
  onNavigate,
  progressMenu,
}: TopBarProps) {
  const codeReview = view?.current === "flows" || view?.current === "files";
  const split = currentPath ? currentPath.lastIndexOf("/") + 1 : 0;
  return (
    <header
      data-code-review={codeReview || undefined}
      className="sticky top-0 z-30 flex min-h-12 flex-none flex-wrap items-center gap-2 border-b border-border bg-surface px-2 py-1 whitespace-nowrap sm:h-12 sm:flex-nowrap sm:gap-3.5 sm:px-3.5 sm:py-0"
    >
      {pr ? (
        <Breadcrumb pr={pr} view={view} prsHref={prsHref} onNavigate={onNavigate} />
      ) : (
        <a href="/" className="font-bold text-text no-underline">
          diff<span className="text-accent">pilot</span>
        </a>
      )}
      {pr && (
        <div
          className={`review-pr-line flex min-w-0 flex-1 items-center gap-2 overflow-hidden ${
            // Separates the PR's own line from the breadcrumb (page path).
            view ? "sm:ml-1.5 sm:border-l-2 sm:border-border sm:pl-5" : ""
          }`}
        >
          {pr.title && (
            <strong title={pr.title} className="review-pr-title min-w-0 truncate font-semibold">
              {pr.title}
            </strong>
          )}
          {codeReview && currentPath && (
            <Tooltip tip={currentPath} className="review-path-tip">
              <code
                className="review-current-path items-baseline text-xs font-normal text-muted"
                role="group"
                aria-label={`Current file: ${currentPath}`}
                tabIndex={0}
              >
                <span className="review-path-dot" aria-hidden>
                  ·&nbsp;
                </span>
                <span className="review-path-directory" aria-hidden>
                  <bdi>{currentPath.slice(0, split)}</bdi>
                </span>
                <bdi className="text-text" aria-hidden>
                  {currentPath.slice(split)}
                </bdi>
              </code>
            </Tooltip>
          )}
          {pr.head_sha && (
            <Tooltip
              tip={`${view ? "Head commit analysed" : "Head commit"}: ${pr.head_sha}`}
              className="shrink-0"
            >
              <code tabIndex={0} className="text-xs text-muted">
                {pr.head_sha.slice(0, 7)}
              </code>
            </Tooltip>
          )}
          <span className="shrink-0">
            <LanguageTag language={language} />
          </span>
          {ci && checksHref && (
            <Tooltip tip={`${CHECKS_LINK_TIP} ${ciTip(ci)}`} className={codeReview ? "shrink-0" : "min-w-0"}>
              <a
                href={checksHref}
                target="_blank"
                rel="noopener noreferrer"
                className={`inline-flex min-h-6 max-w-full items-center rounded border px-1.5 text-[11px] leading-[18px] font-semibold no-underline ${
                  ci.state === "failing" ? "border-high-bd bg-high-bg text-high" : "border-border text-text"
                }`}
              >
                <span className={codeReview ? "review-ci-counts" : "truncate"}>
                  {codeReview ? ciBadgeText(ci) : ciText(ci)}
                </span>
                <GithubMark />
              </a>
            </Tooltip>
          )}
          {ci && !checksHref && (
            <Tooltip tip={ciTip(ci)} className={codeReview ? "shrink-0" : "min-w-0"}>
              <span
                tabIndex={0}
                className={`block max-w-full cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold ${codeReview ? "review-ci-counts" : "truncate"} ${
                  ci.state === "failing" ? "border-high-bd bg-high-bg text-high" : "border-border text-text"
                }`}
              >
                {codeReview ? ciBadgeText(ci) : ciText(ci)}
              </span>
            </Tooltip>
          )}
        </div>
      )}
      {!pr && <span className="flex-1" />}
      <div className="review-controls contents">
        {progressMenu}
        {progress && !progressMenu && (
          <Tooltip tip="Flows you have ticked as reviewed (all their steps ticked, in the Flows view), out of all flows in this PR. No percentage and no weighting by risk.">
            <span tabIndex={0} className="cursor-help text-[12.5px]">
              Your progress <b>{progress.reviewed}</b> / {progress.total}
            </span>
          </Tooltip>
        )}
        {pendingCount !== undefined && (
          <Tooltip
            tip={`Comments in your GitHub pending review for this PR; opens the list.${
              typeof pendingCount === "string" && pendingCount.endsWith("+") ? ` ${PENDING_INCOMPLETE}` : ""
            }`}
          >
            <button type="button" onClick={onOpenPending} className={`review-count-button ${BUTTON}`}>
              Pending review <b>{pendingText(pendingCount)}</b>
            </button>
          </Tooltip>
        )}
        {onHideContextChanges && (
          <Tooltip tip="Hides Rust comment and import changes in the main code view. The classic diff always shows them. Other languages are unchanged.">
            <label className="review-context-toggle flex cursor-pointer items-center gap-1 text-xs whitespace-normal">
              <input
                type="checkbox"
                checked={hideContextChanges}
                onChange={(e) => onHideContextChanges(e.target.checked)}
              />
              <span className="review-context-label">Hide comment and import changes</span>
              <span className="review-context-label-short">
                <span>Hide imports</span>
                <span className="sr-only"> and comment changes</span>
              </span>
            </label>
          </Tooltip>
        )}
      </div>
      {userMenu}
    </header>
  );
}
