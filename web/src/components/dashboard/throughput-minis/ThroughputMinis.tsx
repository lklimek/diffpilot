import { MiniColumnChart } from "../mini-charts/MiniCharts";
import { AsideSection } from "../side-panel/AsideSection";

/** One week of throughput (bucketed by the screen). */
export interface ThroughputWeek {
  /** Week start (`2 Nov`). */
  label: string;
  /** The user's PRs merged. */
  merged: number;
  /** Reviews the scanned logins gave. */
  reviews: number;
  /** This week, still running. */
  partial: boolean;
}

/** PRs merged and reviews given per week (side panel; OD16: no issue statistics). */
export function ThroughputMinis({ weeks }: { weeks: ThroughputWeek[] }) {
  const labels = weeks.map((week) => week.label);
  const lastPartial = weeks.at(-1)?.partial ?? false;
  const lastFull = weeks.at(lastPartial ? -2 : -1);
  return (
    <AsideSection
      title={`Throughput, ${weeks.length} weeks`}
      summary={
        lastFull ? (
          <>
            <b>{lastFull.merged}</b> merged last week
          </>
        ) : null
      }
    >
      <div className="grid gap-3">
        <MiniColumnChart
          title="Your PRs merged"
          note="per week"
          values={weeks.map((week) => week.merged)}
          labels={labels}
          lastPartial={lastPartial}
        />
        <MiniColumnChart
          title="Reviews you gave"
          note="per week"
          values={weeks.map((week) => week.reviews)}
          labels={labels}
          lastPartial={lastPartial}
        />
        {lastPartial && <p className="text-xs text-muted">The last bar is this week so far (lighter).</p>}
      </div>
    </AsideSection>
  );
}
