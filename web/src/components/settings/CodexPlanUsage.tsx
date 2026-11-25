import type { CredentialStatus } from "../../api/generated/types";

function planPercent(value: number): string {
  return `${value}%`; // wording-check: allow provider-observed plan usage (D13)
}

/** Provider-reported plan usage, including when a window reset after the last observation. */
export function CodexPlanUsage({ status, now }: { status?: CredentialStatus; now: Date }) {
  if (status?.state !== "linked") return <p>Plan usage has not been reported.</p>;
  return (
    <>
      {status.paused_until && <p>Plan usage paused until {status.paused_until}.</p>}
      {(["primary", "secondary"] as const).map((window) => {
        const usage = status.telemetry[window];
        const resetPassed = usage?.resets_at != null && Date.parse(usage.resets_at) <= now.getTime();
        return (
          <p key={window} data-codex-plan-usage>
            {window === "primary" ? "Primary" : "Secondary"} plan usage:{" "}
            {resetPassed ? (
              <>reset at {usage.resets_at}; no newer observation</>
            ) : (
              <>
                {usage?.used_percent == null ? "not reported yet" : planPercent(usage.used_percent)}; resets
                at {usage?.resets_at ?? "not reported yet"}
              </>
            )}
            .
          </p>
        );
      })}
    </>
  );
}
