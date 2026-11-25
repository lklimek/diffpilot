import type { WorkItem } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../common/navigate";

/** A Watch chosen on a row: being saved, or saved in this visit. */
export type WatchState = "saving" | "watching";

const WATCH_TIP =
  "diffpilot has not read this pull request's comments. Watch reads them now, then checks for new ones and assesses them with its LLM (uses GitHub requests and LLM tokens).";

/**
 * A pull request's comments in diffpilot (owner 2026-11-23), on ranked and P0 alarm rows: "12 comments · 5 need
 * you" opening its Comments; without counts "Watching, comments not read yet" for a watched one, why for a private
 * one (RW1 reads public repositories only), else Watch.
 */
export function CommentsInfo({
  item,
  href,
  onNavigate,
  onWatch,
  watch,
}: {
  item: WorkItem;
  href?: string;
  onNavigate?: Navigate;
  onWatch?: () => void;
  watch?: WatchState;
}) {
  const subject = `#${item.number ?? ""} ${item.title}`;
  const tally = item.comments;
  if (tally && href) {
    const text = `${tally.total} ${tally.total === 1 ? "comment" : "comments"} · ${tally.needs_you} ${
      tally.needs_you === 1 ? "needs" : "need"
    } you`;
    return (
      <>
        <span aria-hidden="true">·</span>
        <Tooltip tip="Opens the comments of this pull request in diffpilot: counted from its last read; Needs you counts undecided questions for you, including PR-level questions.">
          <a
            href={href}
            aria-label={`${text}: comments of ${subject}`}
            data-comments-link
            onClick={navigateOnClick(href, onNavigate)}
            className="font-semibold text-accent"
          >
            {text}
          </a>
        </Tooltip>
      </>
    );
  }
  if (tally || !onWatch) return null;
  const note = (text: string) => (
    <>
      <span aria-hidden="true">·</span>
      <span data-comments-note className="text-muted">
        {text}
      </span>
    </>
  );
  if (item.watched || watch === "watching") return note("Watching, comments not read yet");
  if (item.private) return note("comments not read: private repository");
  return (
    <Tooltip tip={watch === "saving" ? `${WATCH_TIP} Saving…` : WATCH_TIP}>
      <button
        type="button"
        aria-label={`Watch ${subject}`}
        aria-disabled={watch === "saving" || undefined}
        onClick={() => watch === undefined && onWatch()}
        className="inline-flex min-h-6 cursor-pointer items-center gap-1 rounded-full border border-border bg-surface px-2 text-xs text-text"
      >
        <span aria-hidden="true">○</span>
        Watch
      </button>
    </Tooltip>
  );
}
