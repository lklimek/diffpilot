import type { RefreshStatus } from "../../../api/generated/types";
import { Banner } from "../../banner/Banner";
import { buttonClass, busyTip } from "../../button/button";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { durationText } from "../common/format";
import { ScanSteps } from "../scan-progress/ScanSteps";

interface RefreshErrorProps {
  /** A `failed` refresh. */
  status: RefreshStatus;
  /** Scan time of the snapshot that stays; null before the first. */
  snapshotAt: string | null;
  /** Start a new refresh. */
  onRetry: () => void;
  /** The start request runs. */
  retrying?: boolean;
  now: Date;
}

/** Whether a failed refresh may be retried now (GitHub's rate-limit reset has passed). */
export const canRetry = (status: RefreshStatus, now: Date): boolean =>
  !status.failure?.retry_at || new Date(status.failure.retry_at).getTime() <= now.getTime();

/** A failed refresh (mockup "Error"): what stopped it, that nothing was replaced, when to retry, the scan log. */
export function RefreshError({ status, snapshotAt, onRetry, retrying = false, now }: RefreshErrorProps) {
  const failure = status.failure;
  const retryAt = failure?.retry_at ?? null;
  const ready = canRetry(status, now);
  const kept = snapshotAt
    ? `Nothing was replaced: you are looking at the ${clockText(snapshotAt, now)} snapshot.`
    : "Nothing was stored.";
  const tip = ready
    ? busyTip("Scan GitHub and MemCan again (about 1–2 minutes).", retrying)
    : `Available from ${clockText(retryAt!, now)}, when GitHub resets the limit; retrying earlier would fail again.`;
  return (
    <div className="grid gap-2">
      <Banner
        tone="error"
        title={failure?.message ?? "The refresh failed."}
        actions={
          <Tooltip tip={tip}>
            <button
              type="button"
              aria-disabled={!ready || retrying || undefined}
              onClick={() => ready && !retrying && onRetry()}
              className={buttonClass("plain", true)}
            >
              {ready ? "Retry" : `Retry at ${clockText(retryAt!, now)}`}
            </button>
          </Tooltip>
        }
      >
        {kept}
        {retryAt && !ready && (
          <>
            {" "}
            GitHub allows new requests from <b>{clockText(retryAt, now)}</b> (in{" "}
            {durationText(new Date(retryAt).getTime() - now.getTime())}).
          </>
        )}
        {status.log.length > 0 && (
          <details className="mt-1.5">
            <summary className="cursor-pointer text-xs">Scan log</summary>
            <pre className="mt-1.5 mb-0 font-mono text-xs whitespace-pre-wrap">{status.log.join("\n")}</pre>
          </details>
        )}
      </Banner>
      <ScanSteps steps={status.steps} />
    </div>
  );
}
