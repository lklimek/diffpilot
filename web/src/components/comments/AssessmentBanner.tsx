import { useEffect, useState } from "react";
import type { AssessmentProgress } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { clockText } from "../format/time";

function activeAssessmentText(progress: AssessmentProgress, now: Date): string {
  const { in_pass, waiting, running, since, limited, assessed, total } = progress;
  return [
    in_pass
      ? `Assessing ${in_pass} ${in_pass === 1 ? "comment" : "comments"} now${since ? ` (since ${clockText(since, now)})` : ""}`
      : running && in_pass === undefined
        ? `Assessment pending or running · ${assessed} of ${total} comments assessed`
        : null,
    waiting ? `${waiting} ${waiting === 1 ? "waits" : "wait"} for the next pass` : null,
    limited ? `${limited} not assessed: daily limit` : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function assessmentText(progress: AssessmentProgress, now: Date): string {
  const head = progress.head ? ` at ${progress.head.slice(0, 7)}` : "";
  return (
    activeAssessmentText(progress, now) ||
    `${progress.assessed} of ${progress.total} comments assessed${head}`
  );
}

interface AssessmentBannerProps {
  progress: AssessmentProgress;
  head: string;
  prKey: string;
  now: Date;
  onHide: () => void;
}

export function AssessmentBanner({ progress, head, prKey, now, onHide }: AssessmentBannerProps) {
  const signature = JSON.stringify([
    progress.running,
    progress.in_pass,
    progress.waiting,
    progress.limited,
    head,
  ]);
  return (
    <BannerState
      key={prKey}
      storageKey={`diffpilot:assessment-banner:${prKey}`}
      signature={signature}
      text={activeAssessmentText(progress, now)}
      onHide={onHide}
    />
  );
}

function BannerState({
  storageKey,
  signature,
  text,
  onHide,
}: {
  storageKey: string;
  signature: string;
  text: string;
  onHide: () => void;
}) {
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(storageKey);
    } catch {
      return null;
    }
  });
  if (dismissed && dismissed !== signature) setDismissed(null);
  const visible = dismissed !== signature && Boolean(text);
  useEffect(() => {
    try {
      if (!dismissed) localStorage.removeItem(storageKey);
    } catch {
      // The banner remains dismissible without storage.
    }
  }, [dismissed, storageKey]);
  return (
    <div
      className={
        visible
          ? "mb-3 flex items-center justify-between gap-2 rounded-md border border-border bg-surface px-3 py-1.5 text-sm"
          : "sr-only"
      }
    >
      <p className="m-0" role="status">
        {visible ? text : ""}
      </p>
      {visible && (
        <Tooltip tip="Hide until the assessment state changes">
          <button
            type="button"
            aria-label="Hide"
            className="min-h-6 min-w-6 shrink-0 cursor-pointer rounded hover:bg-surface-2"
            onClick={() => {
              try {
                localStorage.setItem(storageKey, signature);
              } catch {
                // Dismiss for this visit.
              }
              setDismissed(signature);
              onHide();
            }}
          >
            ×
          </button>
        </Tooltip>
      )}
    </div>
  );
}
