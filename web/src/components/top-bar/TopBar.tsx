import { useEffect, useId, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { GithubMark } from "../github/GithubMark";
import { CHECKS_LINK_TIP, ciText, ciTip } from "../section/ci";
import type { CiStatus, LlmLanguage, PrSummary } from "../../api/generated/types";
import { LanguageTag } from "../language-tag/LanguageTag";
import { PENDING_INCOMPLETE } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";

interface TopBarProps {
  hideContextChanges?: boolean;
  onHideContextChanges?: (hide: boolean) => void;
  /** The pull request under review; absent outside the review. */
  pr?: PrSummary;
  /** CI status of the head commit. */
  ci?: CiStatus;
  /** The PR's checks page on GitHub; the CI summary links to it (new tab) when given. */
  checksHref?: string;
  /** Language of the analysis's LLM texts; no tag when absent. */
  language?: LlmLanguage;
  /** Signed-in GitHub login (links to Settings); null when signed out. */
  login: string | null;
  /** Review progress: flows with every step ticked, out of all flows. */
  progress?: { reviewed: number; total: number };
  /** The progress counter as the flows dropdown (both review views); replaces the plain `progress` text. */
  progressMenu?: ReactNode;
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no item when omitted. */
  pendingCount?: number | `${number}+` | "unavailable" | "loading";
  /** Opens the pending-review list. */
  onOpenPending?: () => void;
  /** Opens the `?` help; no button without it. */
  onHelp?: () => void;
  /** The review views of the analysis: with it the bar starts with the breadcrumb and the view menu. */
  view?: ReviewViews;
}

/** The two views of one analysis and where the breadcrumb leads (in-app links). */
export interface ReviewViews {
  current: "flows" | "files";
  flowsHref: string;
  filesHref: string;
  /** The PR context block of the Flows view. */
  prContextHref: string;
  /** Follows an in-app link (a plain click; other clicks keep the browser's behaviour). */
  onNavigate: (href: string) => void;
}

const VIEW_NAMES = { flows: "Flows", files: "Files" } as const;

// A plain left click stays in the app; a modified or middle click opens the link the browser's way.
const inApp = (event: MouseEvent, follow: () => void) => {
  if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  follow();
};

/** `diffpilot › owner/repo #n [GitHub] › Flows ▾ / Files ▾` (brief §4 rule 5). */
function Breadcrumb({ pr, view }: { pr: PrSummary; view: ReviewViews }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const href = { flows: view.flowsHref, files: view.filesHref };
  const sep = (
    <span aria-hidden className="text-muted">
      ›
    </span>
  );
  return (
    <nav aria-label="Breadcrumb" className="flex min-w-0 shrink items-center gap-1.5">
      <a href="/" className="font-bold text-text no-underline">
        diff<span className="text-accent">pilot</span>
      </a>
      {sep}
      <Tooltip tip="Opens the PR context of this review in the Flows view.">
        <a
          href={view.prContextHref}
          onClick={(event) => inApp(event, () => view.onNavigate(view.prContextHref))}
          className="flex min-w-0 items-baseline gap-1 font-semibold text-text no-underline hover:underline"
        >
          <span className="truncate">
            {pr.repo.owner}/{pr.repo.name}
          </span>{" "}
          <span className="shrink-0">#{pr.number}</span>
        </a>
      </Tooltip>
      <Tooltip tip="Open the pull request on GitHub (new tab)">
        <a
          href={pr.html_url}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={`Pull request #${pr.number} on GitHub (opens on GitHub in a new tab)`}
          className="inline-flex size-6 shrink-0 items-center justify-center rounded border border-border"
        >
          <GithubMark />
        </a>
      </Tooltip>
      {sep}
      <div
        ref={box}
        className="relative shrink-0"
        onKeyDown={(event) => {
          if (event.key !== "Escape" || !open) return;
          event.preventDefault();
          event.stopPropagation();
          setOpen(false);
          button.current?.focus();
        }}
      >
        <Tooltip tip="Switch between the two views of this review: Flows (step by step) and Files (one file at a time) (s).">
          <button
            ref={button}
            type="button"
            aria-label={`View: ${VIEW_NAMES[view.current]}`}
            aria-expanded={open}
            aria-controls={`${id}-views`}
            onClick={() => setOpen((o) => !o)}
            className={`${BUTTON} border-accent`}
          >
            {VIEW_NAMES[view.current]} ▾
          </button>
        </Tooltip>
        <ul
          id={`${id}-views`}
          hidden={!open}
          className="absolute top-full left-0 z-40 m-0 mt-1 grid min-w-32 list-none gap-0.5 rounded-md border border-border bg-surface p-1 shadow-tip"
        >
          {(["flows", "files"] as const).map((v) => (
            <li key={v}>
              <a
                href={href[v]}
                aria-current={v === view.current ? "page" : undefined}
                onClick={(event) =>
                  inApp(event, () => {
                    setOpen(false);
                    if (v !== view.current) view.onNavigate(href[v]);
                  })
                }
                className="block min-h-6 rounded px-2 py-0.5 text-text no-underline hover:bg-accent-bg aria-[current=page]:font-bold"
              >
                <span aria-hidden className="inline-block w-4">
                  {v === view.current ? "✓" : ""}
                </span>
                {VIEW_NAMES[v]}
              </a>
            </li>
          ))}
        </ul>
      </div>
    </nav>
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
 * CI status, "Your progress k / n", pending-review count, help, user (link to Settings).
 */
export function TopBar({
  hideContextChanges = true,
  onHideContextChanges,
  pr,
  ci,
  checksHref,
  language,
  login,
  progress,
  pendingCount,
  onOpenPending,
  onHelp,
  view,
  progressMenu,
}: TopBarProps) {
  const repo = pr ? `${pr.repo.owner}/${pr.repo.name}` : "";
  return (
    <header className="sticky top-0 z-30 flex h-12 flex-none items-center gap-3.5 border-b border-border bg-surface px-3.5 whitespace-nowrap">
      {pr && view ? (
        <Breadcrumb pr={pr} view={view} />
      ) : (
        <a href="/" className="font-bold text-text no-underline">
          diff<span className="text-accent">pilot</span>
        </a>
      )}
      {pr && (
        <div
          className={`flex min-w-0 flex-1 items-center gap-2 overflow-hidden ${
            // Separates the PR's own line from the breadcrumb (page path).
            view ? "ml-1.5 border-l-2 border-border pl-5" : ""
          }`}
        >
          {!view && (
            <>
              <a
                href={`https://github.com/${repo}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-muted"
              >
                {repo}
                <GithubMark />
              </a>
              <Tooltip tip="Open the pull request on GitHub (new tab)">
                <a href={pr.html_url} target="_blank" rel="noopener noreferrer" className="font-bold">
                  #{pr.number}
                  <GithubMark />
                </a>
              </Tooltip>
            </>
          )}
          <strong className="min-w-[8ch] truncate font-semibold">{pr.title}</strong>
          <Tooltip tip={`Head commit analysed: ${pr.head_sha}`} className="shrink-0">
            <code tabIndex={0} className="text-xs text-muted">
              {pr.head_sha.slice(0, 7)}
            </code>
          </Tooltip>
          <span className="shrink-0">
            <LanguageTag language={language} />
          </span>
          {ci && checksHref && (
            <Tooltip tip={`${CHECKS_LINK_TIP} ${ciTip(ci)}`} className="min-w-0">
              <a
                href={checksHref}
                target="_blank"
                rel="noopener noreferrer"
                className={`inline-flex min-h-6 max-w-full items-center rounded border px-1.5 text-[11px] leading-[18px] font-semibold no-underline ${
                  ci.state === "failing" ? "border-high-bd bg-high-bg text-high" : "border-border text-text"
                }`}
              >
                <span className="truncate">{ciText(ci)}</span>
                <GithubMark />
              </a>
            </Tooltip>
          )}
          {ci && !checksHref && (
            <Tooltip tip={ciTip(ci)} className="min-w-0">
              <span
                tabIndex={0}
                className={`block max-w-full cursor-help truncate rounded border px-1.5 text-[11px] leading-[18px] font-semibold ${
                  ci.state === "failing" ? "border-high-bd bg-high-bg text-high" : "border-border text-text"
                }`}
              >
                {ciText(ci)}
              </span>
            </Tooltip>
          )}
        </div>
      )}
      {!pr && <span className="flex-1" />}
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
          <button type="button" onClick={onOpenPending} className={BUTTON}>
            Pending review <b>{pendingText(pendingCount)}</b>
          </button>
        </Tooltip>
      )}
      {onHelp && (
        <Tooltip tip="Keyboard shortcuts and how code is drawn (?)">
          <button
            type="button"
            aria-label="Help: keyboard shortcuts and legend"
            onClick={onHelp}
            className={BUTTON}
          >
            ?
          </button>
        </Tooltip>
      )}
      {onHideContextChanges && (
        <Tooltip tip="Hides Rust comment and import changes in the main code view. The classic diff always shows them. Other languages are unchanged.">
          <label className="flex cursor-pointer items-center gap-1 text-xs whitespace-normal">
            <input
              type="checkbox"
              checked={hideContextChanges}
              onChange={(e) => onHideContextChanges(e.target.checked)}
            />
            Hide comment and import changes
          </label>
        </Tooltip>
      )}
      {login && (
        <Tooltip tip={`Settings (signed in as ${login})`}>
          <a href="/settings" aria-label={`Settings, signed in as ${login}`} className="text-muted">
            {login}
          </a>
        </Tooltip>
      )}
    </header>
  );
}
