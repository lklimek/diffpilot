import type { RankingStatus } from "../../../api/generated/types";
import { Banner } from "../../banner/Banner";
import { buttonClass, busyTip } from "../../button/button";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";

interface RankingBannerProps {
  /** The LLM ranking of the current snapshot. */
  ranking: RankingStatus;
  /** When the snapshot's scan finished. */
  scannedAt: string;
  /** Retry ranking (R4); shown when the ranking failed, was interrupted or cancelled. */
  onRetry: () => void;
  /** The retry request runs. */
  retrying?: boolean;
  now: Date;
}

const PROVISIONAL =
  "Items marked provisional show ~N scores: changed items keep their earlier judgement until re-judged, and new items are scored from computed parts and fixed estimates. The order may still change.";

/** Banner of the LLM ranking state (plan §B.3); nothing once it finished. */
export function RankingBanner({ ranking, scannedAt, onRetry, retrying = false, now }: RankingBannerProps) {
  const fresh = `Facts and computed parts are fresh (scan finished ${clockText(scannedAt, now)}).`;
  const retry = (
    <Tooltip
      tip={busyTip(
        "Ask the LLM to rank this snapshot again; only items that still need judging count.",
        retrying,
      )}
    >
      <button
        type="button"
        aria-disabled={retrying || undefined}
        onClick={() => !retrying && onRetry()}
        className={buttonClass("plain", true)}
      >
        Retry ranking
      </button>
    </Tooltip>
  );
  switch (ranking.state) {
    case "waiting_for_analysis":
      return (
        <Banner tone="info" title="Waiting for the review analysis to finish.">
          The LLM ranks this scan when the running review analysis is done. {fresh} {PROVISIONAL}
        </Banner>
      );
    case "running":
      return (
        <Banner
          tone="info"
          title={`Ranking with the LLM: ${ranking.judged} of ${ranking.total} items judged.`}
        >
          {fresh} {PROVISIONAL}
        </Banner>
      );
    case "paused":
      return (
        <Banner tone="warn" title="LLM ranking paused by the usage limit.">
          {ranking.resumes_at
            ? `It continues by itself at ${clockText(ranking.resumes_at, now)}. `
            : "It continues by itself when the limit resets. "}
          {ranking.judged} of {ranking.total} items judged so far. {PROVISIONAL}
        </Banner>
      );
    case "failed":
    case "interrupted":
      return (
        <Banner
          tone="error"
          title={`LLM ranking failed: ${ranking.message ?? (ranking.state === "interrupted" ? "stopped by a server restart" : "no reason given")}. Provisional scores stay approximate.`}
          actions={retry}
        >
          {fresh}
        </Banner>
      );
    case "cancelled":
      return (
        <Banner
          tone="warn"
          title="LLM ranking cancelled. Provisional scores stay approximate."
          actions={retry}
        >
          {fresh}
        </Banner>
      );
    default:
      return null;
  }
}
