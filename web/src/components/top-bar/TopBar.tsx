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
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no item when omitted. */
  pendingCount?: number | `${number}+` | "unavailable" | "loading";
  /** Opens the pending-review list. */
  onOpenPending?: () => void;
  /** Opens the `?` help; no button without it. */
  onHelp?: () => void;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-[12.5px] font-semibold text-text hover:border-accent";

function pendingText(count: NonNullable<TopBarProps["pendingCount"]>): string {
  if (count === "unavailable") return "not available";
  if (count === "loading") return "…";
  return String(count);
}

/**
 * Top bar (brief §4 rule 5): brand, pull request (number opens GitHub in a new tab), head SHA, CI status,
 * "flows reviewed k / n", pending-review count, help, user (link to Settings).
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
}: TopBarProps) {
  const repo = pr ? `${pr.repo.owner}/${pr.repo.name}` : "";
  return (
    <header className="sticky top-0 z-30 flex h-12 flex-none items-center gap-3.5 border-b border-border bg-surface px-3.5 whitespace-nowrap">
      <a href="/" className="font-bold text-text no-underline">
        diff<span className="text-accent">pilot</span>
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
          <LanguageTag language={language} />
          {ci && checksHref && (
            <Tooltip tip={`${CHECKS_LINK_TIP} ${ciTip(ci)}`}>
              <a
                href={checksHref}
                target="_blank"
                rel="noopener noreferrer"
                className={`rounded border px-1.5 text-[11px] leading-[18px] font-semibold no-underline ${
                  ci.state === "failing" ? "border-high-bd bg-high-bg text-high" : "border-border text-text"
                }`}
              >
                {ciText(ci)} ↗
              </a>
            </Tooltip>
          )}
          {ci && !checksHref && (
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
