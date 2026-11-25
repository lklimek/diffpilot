import { RANKING_DAILY_LIMIT, type Judgement } from "../../../api/provisional/dashboardJudgement";
import { Tooltip } from "../../tooltip/Tooltip";
import { dayText } from "../common/format";
import { navigateOnClick, type Navigate } from "../common/navigate";

const MARK = "inline-flex min-h-6 items-center rounded border border-dashed px-1.5 text-xs font-semibold";

export function JudgementMark({
  judgement,
  onRejudge,
  usageHref,
  onNavigate,
}: {
  judgement: Judgement;
  onRejudge?: () => void;
  usageHref?: string;
  onNavigate?: Navigate;
}) {
  const { state, judged_at, outdated_because } = judgement;
  if (state === "current") return null;
  if (state === "outdated") {
    const date = judged_at ? dayText(judged_at) : null;
    const reason =
      outdated_because === "prompt_changed"
        ? "The ranking prompt changed"
        : outdated_because === "facts_changed"
          ? "Facts changed"
          : "Judgement inputs changed";
    const tip = `${reason} after the LLM judged this${date ? ` on ${date}` : ""}. ${
      onRejudge
        ? `Click to judge this item again; uses 1 of today’s ${RANKING_DAILY_LIMIT}.`
        : "Re-judging individual items is unavailable."
    }`;
    return (
      <Tooltip tip={tip}>
        <button
          type="button"
          aria-disabled={!onRejudge || undefined}
          onClick={onRejudge}
          className={`${MARK} border-med-bd text-med hover:bg-med-bg aria-disabled:cursor-not-allowed`}
        >
          <span aria-hidden className="mr-1">
            ↻
          </span>
          outdated{date ? ` · ${date}` : ""}
        </button>
      </Tooltip>
    );
  }
  const [text, tip] =
    state === "judging"
      ? [
          "◔ judging…",
          "The LLM is judging this item. Any earlier judgement remains visible until it finishes.",
        ]
      : state === "queued"
        ? ["◌ not judged yet · queued", "This item is queued for LLM judgement."]
        : state === "capped"
          ? ["◌ not judged · daily limit", "The daily LLM ranking limit has been reached."]
          : ["◌ not judged yet", "Scored from computed parts and fixed estimates until the LLM judges it."];
  return (
    <>
      <Tooltip tip={tip}>
        <span
          tabIndex={0}
          className={`${MARK} cursor-help ${state === "judging" ? "border-accent bg-accent-bg text-accent" : "border-med-bd text-med"}`}
        >
          {text}
        </span>
      </Tooltip>
      {state === "capped" && usageHref && (
        <>
          {" "}
          <a
            href={usageHref}
            onClick={navigateOnClick(usageHref, onNavigate)}
            className="inline-flex min-h-6 items-center text-accent underline"
          >
            Usage and limits
          </a>
        </>
      )}
    </>
  );
}
