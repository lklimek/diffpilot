import type { WorkItem } from "../../../api/generated/types";
import { buttonClass } from "../../button/button";
import { GithubMark } from "../../github/GithubMark";
import { isGithubUrl } from "../../github/links";
import { Tooltip } from "../../tooltip/Tooltip";
import { ExternalIcon } from "../common/icons";
import { navigateOnClick, type Navigate } from "../common/navigate";
import { httpUrl } from "../common/OpenExternal";

const WIDTH = "min-w-[118px] justify-center";

/** Label and tooltip of a pull request's review link (plan §F.1). */
export function reviewAction(item: WorkItem): { label: string; tip: string; primary: boolean } {
  const review = item.review;
  if (!review) {
    return {
      label: "Review",
      tip: "Opens the review start page with this PR's URL filled in. Starts nothing until you click Start analysis.",
      primary: false,
    };
  }
  if (review.head_moved) {
    return {
      label: "Open",
      tip: "The PR moved. Opens the start page; starts nothing.",
      primary: true,
    };
  }
  return {
    label: "Open",
    tip: review.has_my_state
      ? "Opens your review; ticks and answers are kept. Starts nothing."
      : "Opens your review. Starts nothing.",
    primary: true,
  };
}

interface ItemActionProps {
  item: WorkItem;
  /** In-app href of a pull request's review (built by the screen, plan §F.1); unused for issues and TODOs. */
  reviewHref?: string;
  /** In-app Comments href, built by the screen. */
  commentsHref?: string;
  onNavigate?: Navigate;
}

/** The action of an item: a PR's review link, or the issue / TODO in a new tab; nothing for a TODO without a link. */
export function ItemAction({ item, reviewHref, commentsHref, onNavigate }: ItemActionProps) {
  if (item.kind === "pr") {
    return (
      <span className="flex flex-col gap-1">
        {reviewHref && <ReviewLink item={item} href={reviewHref} onNavigate={onNavigate} />}
        {commentsHref && (
          <Tooltip tip="Opens Comments. Reads the PR from GitHub unless there is a current read, then assesses comments needing assessment with the LLM (uses GitHub requests and LLM tokens).">
            <a
              href={commentsHref}
              aria-label={`Comments of ${item.id}`}
              onClick={navigateOnClick(commentsHref, onNavigate)}
              className={`${buttonClass("plain", true)} ${WIDTH}`}
            >
              Comments
            </a>
          </Tooltip>
        )}
      </span>
    );
  }
  const url = httpUrl(item.url);
  if (!url) return null;
  const what = item.kind === "todo" ? "TODO" : "issue";
  return (
    <Tooltip tip={`Open the ${what} in ${item.kind === "todo" ? "MemCan" : "GitHub"} (new tab)`}>
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className={`${buttonClass("plain", true)} ${WIDTH}`}
      >
        Open {what} {isGithubUrl(url) ? <GithubMark /> : <ExternalIcon />}
      </a>
    </Tooltip>
  );
}

function ReviewLink({ item, href, onNavigate }: { item: WorkItem; href: string; onNavigate?: Navigate }) {
  const { label, tip, primary } = reviewAction(item);
  // Unique per row and starting with the visible label (WCAG 2.5.3); aria-describedby is taken by the tooltip.
  const subject = `#${item.number ?? ""} ${item.title}`;
  const name = label === "Open" ? `Open review of ${subject}` : `Review pull request ${subject}`;
  return (
    <Tooltip tip={tip}>
      <a
        href={href}
        aria-label={name}
        data-review-link
        onClick={navigateOnClick(href, onNavigate)}
        className={`${buttonClass(primary ? "primary" : "plain", true)} ${WIDTH}`}
      >
        {label}
      </a>
    </Tooltip>
  );
}
