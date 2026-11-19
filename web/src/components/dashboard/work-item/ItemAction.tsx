import type { WorkItem } from "../../../api/generated/types";
import { buttonClass } from "../../button/button";
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
      tip: `The PR has moved since your analysis (head ${review.analysed_head.slice(0, 7)}). Opens the start page, which offers to re-analyse or open the earlier review. Starts nothing.`,
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
  onNavigate?: Navigate;
}

/** The action of an item: a PR's review link, or the issue / TODO in a new tab; nothing for a TODO without a link. */
export function ItemAction({ item, reviewHref, onNavigate }: ItemActionProps) {
  if (item.kind === "pr" && reviewHref) {
    const { label, tip, primary } = reviewAction(item);
    // Unique per row and starting with the visible label (WCAG 2.5.3); aria-describedby is taken by the tooltip.
    const subject = `#${item.number ?? ""} ${item.title}`;
    const name = label === "Open" ? `Open review of ${subject}` : `Review pull request ${subject}`;
    return (
      <Tooltip tip={tip}>
        <a
          href={reviewHref}
          aria-label={name}
          data-review-link
          onClick={navigateOnClick(reviewHref, onNavigate)}
          className={`${buttonClass(primary ? "primary" : "plain", true)} ${WIDTH}`}
        >
          {label}
        </a>
      </Tooltip>
    );
  }
  const url = httpUrl(item.url);
  if (item.kind === "pr" || !url) return null;
  const what = item.kind === "todo" ? "TODO" : "issue";
  return (
    <Tooltip tip={`Open the ${what} in ${item.kind === "todo" ? "MemCan" : "GitHub"} (new tab)`}>
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className={`${buttonClass("plain", true)} ${WIDTH}`}
      >
        Open {what} <ExternalIcon />
      </a>
    </Tooltip>
  );
}
