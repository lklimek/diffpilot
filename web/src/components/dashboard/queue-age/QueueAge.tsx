import { daysSince } from "../common/format";
import { MiniBarChart } from "../mini-charts/MiniCharts";
import { AsideSection } from "../side-panel/AsideSection";

/** Age buckets of a waiting review, in days. */
export const AGE_BUCKETS = ["≤ 2 d", "3–7 d", "8–14 d", "> 14 d"] as const;

/** Counts of `since` times per bucket of {@link AGE_BUCKETS}, in whole days at `now`. */
export function ageBuckets(since: string[], now: Date): number[] {
  const counts = [0, 0, 0, 0];
  for (const iso of since) {
    const days = daysSince(iso, now);
    counts[days <= 2 ? 0 : days <= 7 ? 1 : days <= 14 ? 2 : 3]! += 1;
  }
  return counts;
}

interface QueueAgeProps {
  /** Your PRs waiting on others, per bucket. */
  yoursWaiting: number[];
  /** PRs waiting on your review, per bucket; null when not known. */
  waitingOnYou: number[] | null;
}

const sum = (counts: number[]) => counts.reduce((a, b) => a + b, 0);

/** How long reviews have waited, by age bucket; the bars share one scale (side panel). */
export function QueueAge({ yoursWaiting, waitingOnYou }: QueueAgeProps) {
  const max = Math.max(1, ...yoursWaiting, ...(waitingOnYou ?? []));
  return (
    <AsideSection
      title="Review queue age"
      summary={
        <>
          {waitingOnYou && (
            <>
              <b>{sum(waitingOnYou)}</b> wait on you ·{" "}
            </>
          )}
          <b>{sum(yoursWaiting)}</b> of yours wait
        </>
      }
    >
      <div className="grid gap-3">
        {waitingOnYou && (
          <MiniBarChart
            title="PRs waiting on your review"
            values={waitingOnYou}
            labels={[...AGE_BUCKETS]}
            max={max}
          />
        )}
        <MiniBarChart
          title="Your PRs waiting on others"
          values={yoursWaiting}
          labels={[...AGE_BUCKETS]}
          max={max}
        />
        <p className="text-xs text-muted">Age since the review was requested. Bars share one scale.</p>
      </div>
    </AsideSection>
  );
}
