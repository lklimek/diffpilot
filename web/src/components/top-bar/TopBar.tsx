import type { PrSummary } from "../../api/generated/types";
import type { CiFact } from "../pr-context/PrContextBlock";
import { Tooltip } from "../tooltip/Tooltip";

interface TopBarProps {
  /** The pull request under review; absent outside the review. */
  pr?: PrSummary;
  /** CI status of the head commit. */
  ci?: CiFact;
  /** Signed-in GitHub login (links to Settings); null when signed out. */
  login: string | null;
  /** Review progress: flows with every step ticked, out of all flows. */
  progress?: { reviewed: number; total: number };
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no item when omitted. */
  pendingCount?: number | "unavailable" | "loading";
  /** Opens the pending-review list. */
  onOpenPending?: () => void;
  /** Opens the `?` help; no button without it. */
  onHelp?: () => void;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-[12.5px] font-semibold text-text hover:border-accent";

function ciText(ci: CiFact): string {
  switch (ci.state) {
    case "failing":
      return `CI: ${ci.failing.length} of ${ci.total} failing`;
    case "passing":
      return `CI: ${ci.total} passing`;
    case "pending":
      return "CI: running";
    case "none":
      return "CI: no checks";
    case "not_checked":
      return "CI: not checked";
  }
}

function ciTip(ci: CiFact): string {
  if (ci.state === "not_checked") return "The CI checks of the head commit could not be read from GitHub.";
  if (ci.failing.length === 0) return "CI status of the head commit, from GitHub.";
  return `Failing checks: ${ci.failing.map((c) => c.name).join(", ")}. From GitHub.`;
}

function pendingText(count: NonNullable<TopBarProps["pendingCount"]>): string {
  if (count === "unavailable") return "not available";
  if (count === "loading") return "…";
  return String(count);
}

/**
 * Top bar (brief §4 rule 5): brand, pull request (number opens GitHub in a new tab), head SHA, CI status,
 * "flows reviewed k / n", pending-review count, help, user (link to Settings).
 */
export function TopBar({ pr, ci, login, progress, pendingCount, onOpenPending, onHelp }: TopBarProps) {
  const repo = pr ? `${pr.repo.owner}/${pr.repo.name}` : "";
  return (
    <header className="sticky top-0 z-30 flex h-12 flex-none items-center gap-3.5 border-b border-border bg-surface px-3.5 whitespace-nowrap">
      <a href="/" className="font-bold text-text no-underline">
        smart<span className="text-accent">-</span>review
      </a>
      {pr && (
        <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden">
          <a
            href={`https://github.com/${repo}`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-muted"
          >
            {repo}
          </a>
          <Tooltip tip="Open the pull request on GitHub (new tab)">
            <a href={pr.html_url} target="_blank" rel="noopener noreferrer" className="font-bold">
              #{pr.number} ↗
            </a>
          </Tooltip>
          <strong className="truncate font-semibold">{pr.title}</strong>
          <Tooltip tip={`Head commit analysed: ${pr.head_sha}`}>
            <code tabIndex={0} className="text-xs text-muted">
              {pr.head_sha.slice(0, 7)}
            </code>
          </Tooltip>
          {ci && (
            <Tooltip tip={ciTip(ci)}>
              <span
                tabIndex={0}
                className={`cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold ${
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
      {progress && (
        <Tooltip tip="Flows you have ticked as reviewed (all their steps ticked), out of all flows in this PR. No percentage and no weighting by risk.">
          <span tabIndex={0} className="cursor-help text-[12.5px]">
            flows reviewed <b>{progress.reviewed}</b> / {progress.total}
          </span>
        </Tooltip>
      )}
      {pendingCount !== undefined && (
        <Tooltip tip="Comments in your GitHub pending review for this PR; opens the list.">
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
