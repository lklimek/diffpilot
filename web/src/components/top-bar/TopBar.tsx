import type { CiStatus, PrSummary } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

interface TopBarProps {
  /** The pull request under review; absent outside the review. */
  pr?: PrSummary;
  /** CI status of the head commit. */
  ci?: CiStatus;
  /** Signed-in GitHub login; null when signed out. */
  login: string | null;
}

function ciText(ci: CiStatus): string {
  switch (ci.state) {
    case "failing":
      return `CI: ${ci.failing.length} of ${ci.total} failing`;
    case "passing":
      return `CI: ${ci.total} passing`;
    case "pending":
      return "CI: running";
    case "none":
      return "CI: no checks";
  }
}

function ciTip(ci: CiStatus): string {
  if (ci.failing.length === 0) return "CI status of the head commit, from GitHub.";
  return `Failing checks: ${ci.failing.map((c) => c.name).join(", ")}. From GitHub.`;
}

/** Top bar: brand, pull request (number opens GitHub in a new tab), head SHA, CI status, user. */
export function TopBar({ pr, ci, login }: TopBarProps) {
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
      {login && <span className="text-muted">{login}</span>}
    </header>
  );
}
