import { useId, useRef, useState } from "react";
import type { RefreshStatus } from "../../../api/generated/types";
import { buttonClass, busyTip } from "../../button/button";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { ScanSteps, scanStepLabel } from "./ScanSteps";

interface ScanProgressProps {
  status: RefreshStatus;
  snapshotAt: string | null;
  onCancel: () => void;
  cancelling?: boolean;
  now: Date;
}

export function ScanProgress({ status, snapshotAt, onCancel, cancelling = false, now }: ScanProgressProps) {
  const titleId = useId();
  const detailsId = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const step = status.steps.find((step) => step.state === "running");
  const label = step ? scanStepLabel(step.key) : null;
  const counts =
    step?.done != null && step.total != null && step.total > 0
      ? { done: step.done, total: step.total }
      : null;
  const kept = snapshotAt ? `the ${clockText(snapshotAt, now)} snapshot stays` : "the current list stays";
  return (
    <section
      aria-labelledby={titleId}
      className="grid gap-2 rounded-lg border border-border bg-surface px-3 py-2"
      onKeyDown={(event) => {
        if (!open || event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }}
    >
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        <h2 id={titleId} className="text-sm font-semibold">
          Refreshing from GitHub and MemCan
        </h2>
        <span className="text-xs text-muted">
          <span
            role="progressbar"
            aria-label="Scan progress"
            aria-valuenow={counts?.done}
            aria-valuemax={counts?.total}
            aria-valuetext={counts ? `${label}: ${counts.done} of ${counts.total}` : (label ?? undefined)}
          >
            {`· ${label ?? "Scanning"}`}
            {counts && ` ${counts.done} / ${counts.total}`}
          </span>{" "}
          · started {clockText(status.started_at, now)}
        </span>
        <div className="ml-auto flex gap-1.5">
          <Tooltip tip="Show or hide every scan step.">
            <button
              ref={trigger}
              type="button"
              aria-expanded={open}
              aria-controls={detailsId}
              onClick={() => setOpen(!open)}
              className={buttonClass("plain", true)}
            >
              Details
            </button>
          </Tooltip>
          <Tooltip tip={busyTip(`Stops the scan; ${kept}.`, cancelling)}>
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
      </div>
      <div id={detailsId} hidden={!open}>
        {open && <ScanSteps steps={status.steps} />}
      </div>
    </section>
  );
}
