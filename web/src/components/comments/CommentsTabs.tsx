import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";

/** A page of the Comments view. */
export type CommentsPage = "questions" | "table" | "rounds" | "carry-overs" | "log";

const NAMES: Record<CommentsPage, string> = {
  questions: "Questions",
  table: "Table",
  rounds: "Rounds",
  "carry-overs": "Carry-overs",
  log: "Decision log and agents",
};

interface CommentsTabsProps {
  current: CommentsPage;
  hrefs: Record<CommentsPage, string>;
  /** Undecided questions (on Questions). */
  questions: number;
  onNavigate: Navigate;
}

/** The pages of Comments (V1–V5) as links; the current one is marked. */
export function CommentsTabs({ current, hrefs, questions, onNavigate }: CommentsTabsProps) {
  return (
    <nav aria-label="Comments pages" className="mb-3 flex flex-wrap gap-1 border-b border-border">
      {(Object.keys(NAMES) as CommentsPage[]).map((page) => (
        <a
          key={page}
          href={hrefs[page]}
          aria-current={page === current ? "page" : undefined}
          onClick={navigateOnClick(hrefs[page], onNavigate)}
          className="-mb-px inline-flex min-h-6 items-center gap-1.5 border-b-[3px] border-transparent px-3 py-1.5 text-text no-underline aria-[current=page]:border-accent aria-[current=page]:font-semibold"
        >
          {NAMES[page]}
          {page === "questions" && (
            <span className="rounded-full bg-accent px-1.5 text-xs text-on-accent">{questions}</span>
          )}
        </a>
      ))}
    </nav>
  );
}
