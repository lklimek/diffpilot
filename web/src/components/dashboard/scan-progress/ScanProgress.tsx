import { useId } from "react";
import type { RefreshStatus } from "../../../api/generated/types";
import { buttonClass, busyTip } from "../../button/button";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { ScanSteps } from "./ScanSteps";

interface ScanProgressProps {
  /** A refresh in `scanning`. */
  status: RefreshStatus;
  /** Scan time of the snapshot the list keeps meanwhile; null before the first. */
  snapshotAt: string | null;
  onCancel: () => void;
  /** The cancel request runs. */
  cancelling?: boolean;
  now: Date;
}

/** The running scan (mockup "Refreshing" card): progress bar, steps, Cancel scan. */
export function ScanProgress({ status, snapshotAt, onCancel, cancelling = false, now }: ScanProgressProps) {
  const titleId = useId();
  const percent = Math.max(0, Math.min(100, status.percent));
  const kept = snapshotAt ? `the ${clockText(snapshotAt, now)} snapshot` : "the list";
  return (
    <section
      aria-labelledby={titleId}
      className="grid gap-2.5 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <div className="flex flex-wrap items-center gap-3">
        <h2 id={titleId} className="text-[15px] font-semibold">
          Refreshing from GitHub and MemCan
        </h2>
        <span className="text-xs text-muted">usually 1–2 minutes · read-only</span>
        <span className="flex-1" />
        <Tooltip tip={busyTip(`Stops the scan; ${kept} stays.`, cancelling)}>
          <button
            type="button"
            aria-disabled={cancelling || undefined}
            onClick={() => !cancelling && onCancel()}
            className={buttonClass("plain", true)}
          >
            Cancel scan
          </button>
        </Tooltip>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <div
          role="progressbar"
          aria-label="Scan progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          className="h-2.5 min-w-[200px] flex-1 overflow-hidden rounded-full border border-border bg-surface-2"
        >
          <i
            className="block h-full origin-left bg-accent transition-transform duration-500"
            style={{ transform: `scaleX(${percent / 100})` }}
          />
        </div>
        <span className="font-mono text-xs">
          {percent}% {/* wording-check: allow scan progress (ADR-016), not a confidence */}
        </span>
      </div>
      <ScanSteps steps={status.steps} />
      <p className="text-xs text-muted">
        The list below is {kept} until the scan finishes; then items the LLM has not judged yet are marked
        provisional.
      </p>
    </section>
  );
}
