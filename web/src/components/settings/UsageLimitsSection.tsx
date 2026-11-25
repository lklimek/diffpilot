import { Tooltip } from "../tooltip/Tooltip";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import type {
  AssessmentQuota,
  CredentialStatus,
  LlmCallOutcome,
  UsageTotals,
  UsageWindows,
} from "../../api/generated/types";
import type { RankingUsage } from "../../api/provisional/rankingUsage";
import { buttonClass } from "../button/button";
import { dateTimeText } from "../format/time";
import { useAction } from "../inspector/action";
import { UsageLine } from "../usage/UsageLine";
import { CodexPlanUsage } from "./CodexPlanUsage";
import { Section } from "./Section";

function Tile({ title, children }: { title: string; children: ReactNode }) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className="grid min-w-0 content-start gap-1 rounded-lg border border-border bg-surface-2 p-3 text-sm"
    >
      <h3 id={id} className="font-semibold">
        {title}
      </h3>
      {children}
    </section>
  );
}

function resetTime(at: string, zone?: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return "an unknown time";
  if (zone) {
    try {
      return `${new Intl.DateTimeFormat("en-GB", { timeStyle: "short", timeZone: zone }).format(date)} (${zone})`;
    } catch (error) {
      if (!(error instanceof RangeError)) throw error;
    }
  }
  return dateTimeText(at);
}

/** Usage based on server counts and the last observations from the provider. */
export function UsageLimitsSection({
  ranking,
  rankingError,
  rankingZone,
  onReset,
  quota,
  quotaError,
  tokens,
  month,
  monthError,
  codex,
  now,
  lastCall,
}: {
  ranking?: RankingUsage | null;
  rankingError?: string;
  rankingZone?: string;
  onReset: () => Promise<unknown>;
  quota?: AssessmentQuota;
  quotaError?: string;
  tokens?: UsageWindows;
  month?: UsageTotals | null;
  monthError?: string;
  codex?: CredentialStatus;
  now: Date;
  lastCall: LlmCallOutcome | null;
}) {
  const [confirm, setConfirm] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const action = useAction();
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (confirm) cancel.current?.focus();
  }, [confirm]);
  const close = () => {
    setConfirm(false);
    trigger.current?.focus();
  };
  const reset = async () => {
    if (await action.run(onReset)) close();
  };
  return (
    <Section title="Usage and limits">
      <div className="grid grid-cols-1 @min-[560px]/settings:grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-3">
        <Tile title="Ranking today">
          {ranking ? (
            <>
              <p className="font-semibold">
                {ranking.sent_today} of {ranking.daily_cap} items sent for judgement
              </p>
              <meter
                aria-label="Items sent for judgement today"
                aria-valuetext={`${ranking.sent_today} of ${ranking.daily_cap} items`}
                value={ranking.sent_today}
                max={Math.max(1, ranking.daily_cap)}
                className="w-full accent-accent"
              />
              <p>Up to {ranking.per_refresh_cap} per refresh</p>
              <p>Resets at {resetTime(ranking.resets_at, rankingZone)}</p>
              <p>Left provisional by a limit: {ranking.left_provisional}</p>
              <Tooltip tip="Shows what will be reset before you confirm.">
                <button
                  ref={trigger}
                  type="button"
                  aria-expanded={confirm}
                  className={`${buttonClass("plain")} w-fit`}
                  onClick={() => {
                    action.setError(null);
                    setConfirm(true);
                  }}
                >
                  Reset my limits
                </button>
              </Tooltip>
              {confirm && (
                <div
                  role="group"
                  aria-label="Confirm ranking limit reset"
                  className="grid gap-1"
                  onKeyDown={(event) => {
                    if (event.key === "Escape" && !action.busy) close();
                  }}
                >
                  <p>
                    Sets today’s count to 0 for you; a running ranking continues; the per-refresh cap stays.
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Tooltip tip="Keeps today’s ranking count.">
                      <button
                        type="button"
                        ref={cancel}
                        aria-disabled={action.busy || undefined}
                        className={buttonClass("plain")}
                        onClick={() => !action.busy && close()}
                      >
                        Cancel
                      </button>
                    </Tooltip>
                    <Tooltip tip="Sets your count for today to zero.">
                      <button
                        type="button"
                        aria-disabled={action.busy || undefined}
                        className={buttonClass("plain")}
                        onClick={() => void reset()}
                      >
                        Reset today&apos;s count
                      </button>
                    </Tooltip>
                  </div>
                </div>
              )}
              {action.error && <p role="alert">{action.error}</p>}
            </>
          ) : rankingError ? (
            <p role="alert">Could not load ranking usage. {rankingError}</p>
          ) : (
            <p role="status">Not available yet</p>
          )}
        </Tile>
        <Tile title="Comment assessment">
          {quota ? (
            <>
              <p>{80 - quota.remaining} of 80 passes today</p>
              <p>Resets at {dateTimeText(quota.resets_at)}</p>
            </>
          ) : quotaError ? (
            <p role="alert">{quotaError}</p>
          ) : (
            <p role="status">Loading comment assessment usage…</p>
          )}
        </Tile>
        <Tile title="Tokens">
          {(
            [
              ["Today", tokens?.today],
              ["Last 7 days", tokens?.last_7_days],
            ] as const
          ).map(([label, usage]) =>
            usage?.jobs ? (
              <UsageLine key={label} label={label} usage={usage} />
            ) : (
              <p key={label}>{label}: not available yet</p>
            ),
          )}
          {monthError ? (
            <p role="alert">Could not load monthly token usage. {monthError}</p>
          ) : month?.jobs ? (
            <UsageLine label="This month" usage={month} />
          ) : (
            <p role="status">This month: {month ? "0 jobs with token reports" : "not available yet"}</p>
          )}
        </Tile>
        <Tile title="Codex plan">
          <p className="text-xs text-muted">Reported by the provider.</p>
          <CodexPlanUsage status={codex} now={now} />
        </Tile>
        <Tile title="Last LLM call">
          {lastCall ? (
            <>
              <p>
                {lastCall.status.state === "working" ? "Succeeded" : "Failed"} · {dateTimeText(lastCall.at)}
              </p>
              {lastCall.status.state === "not_working" && <p>{lastCall.status.message}</p>}
            </>
          ) : (
            <p>No LLM call yet.</p>
          )}
        </Tile>
      </div>
    </Section>
  );
}
