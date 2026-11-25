import type { WorkItem } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../common/navigate";

/**
 * A pull request's comments in diffpilot (owner 2026-11-23), on ranked and P0 alarm rows: "12 comments · 5 need
 * you" opening its Comments; without counts "Watching, comments not read yet" for a watched one, why for a private
 * one (RW1 reads public repositories only).
 */
export function CommentsInfo({
  item,
  href,
  onNavigate,
  watched,
}: {
  item: WorkItem;
  href?: string;
  onNavigate?: Navigate;
  watched?: boolean;
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
  if (tally) return null;
  const note = (text: string) => (
    <>
      <span aria-hidden="true">·</span>
      <span data-comments-note className="text-muted">
        {text}
      </span>
    </>
  );
  if (watched) return note("Watching, comments not read yet");
  if (item.private) return note("comments not read: private repository");
  return null;
}
