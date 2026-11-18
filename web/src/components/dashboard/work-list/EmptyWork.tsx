import { useId } from "react";
import type { KindCounts } from "../../../api/generated/types";
import { buttonClass } from "../../button/button";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../common/navigate";

interface EmptyWorkProps {
  /** When the snapshot's scan finished. */
  checkedAt: string;
  /** What the scan read before exclusion. */
  checked: KindCounts;
  /** Items left out of the ranking. */
  excluded: number;
  /** The user's PRs waiting on others. */
  awaiting: number;
  /** In-app href of the review start page. */
  reviewStartHref: string;
  onNavigate?: Navigate;
  now: Date;
}

/** The list when nothing is ranked (mockup "Empty"): what was checked, and a way to review any PR. */
export function EmptyWork({
  checkedAt,
  checked,
  excluded,
  awaiting,
  reviewStartHref,
  onNavigate,
  now,
}: EmptyWorkProps) {
  const titleId = useId();
  return (
    <section
      aria-labelledby={titleId}
      className="grid gap-2 rounded-lg border border-dashed border-border bg-surface px-6 py-7"
    >
      <h2 id={titleId} className="text-lg font-semibold">
        Nothing needs you right now.
      </h2>
      <p>
        No pull request waits for your review, nothing is assigned to you
        {awaiting > 0 ? `, and ${awaiting} of your PRs are with their reviewers (see Waiting on others)` : ""}
        .
      </p>
      <p className="text-xs text-muted">
        Checked at {clockText(checkedAt, now)}: {checked.pr} PRs, {checked.issue} issues and {checked.todo}{" "}
        open TODOs.
        {excluded > 0
          ? ` Excluded: ${excluded} (drafts of other authors, skipped bot authors, no owned change).`
          : ""}
      </p>
      <p>
        <Tooltip tip="Open any pull request by URL">
          <a
            href={reviewStartHref}
            onClick={navigateOnClick(reviewStartHref, onNavigate)}
            className={buttonClass("plain")}
          >
            Review a PR by URL
          </a>
        </Tooltip>
      </p>
    </section>
  );
}
