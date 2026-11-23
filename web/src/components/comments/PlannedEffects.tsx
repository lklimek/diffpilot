import { useEffect, useRef, type ReactNode } from "react";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";
import type { DecisionBatchRow } from "../../api/generated/types";
import { effectLines, executorName, previewFor, type EffectsPreviewState } from "./decisions";

/** Reports a card's draft to `onDraft` whenever its content changes (the screen has the server plan it, RC16). */
export function useDraft(
  rows: DecisionBatchRow[] | null,
  onDraft?: (rows: DecisionBatchRow[] | null) => void,
) {
  const latest = useRef(onDraft);
  useEffect(() => {
    latest.current = onDraft;
  });
  const key = JSON.stringify(rows);
  useEffect(() => latest.current?.(JSON.parse(key) as DecisionBatchRow[] | null), [key]);
}

interface PlannedEffectsProps {
  /** The server's plan of the card's draft (RC16). */
  preview: EffectsPreviewState | undefined;
  /** The items the card decides. */
  nos: number[];
  /** Why nothing is planned yet (an input is missing). */
  idle: string;
  /** The effects act on several items (named in the lines); default: when `nos` has several. */
  several?: boolean;
  /** Lines after the effects (thread left open, comment sent to the agent, …). */
  children?: ReactNode;
}

/** "What happens next": the effects the server plans for the card's draft, in order, with their executor. */
export function PlannedEffects({
  preview,
  nos,
  idle,
  several = nos.length > 1,
  children,
}: PlannedEffectsProps) {
  const planned = nos.map((no) => previewFor(preview, no));
  const effects = planned.every((p) => p !== null) ? planned.flat() : null;
  const status =
    preview?.state === "error"
      ? preview.message
      : preview === undefined || preview.state === "idle"
        ? idle
        : effects === null
          ? "Planning what happens…"
          : effects.length === 0
            ? "Nothing is posted or changed on GitHub."
            : null;
  return (
    <>
      <ol
        aria-busy={preview?.state === "ready" && preview.updating}
        className="m-0 grid gap-1 pl-5 text-[13px]"
      >
        {effects !== null &&
          effectLines(effects, several).map((line, i) => (
            <li key={i}>
              <span className="mr-1.5 rounded bg-accent-bg px-1 text-[11px] font-bold text-accent">
                {executorName(line.executor)}
              </span>
              {line.text}
            </li>
          ))}
        {children}
      </ol>
      {status && (
        <p
          role={preview?.state === "error" ? "alert" : "status"}
          className={`m-0 mt-1 text-xs ${preview?.state === "error" ? "text-high" : "text-muted"}`}
        >
          {status}
        </p>
      )}
    </>
  );
}

/**
 * Why Decide is unavailable (`reason`), announced as it changes; the Decide button names it by `id`. A failed plan
 * (no answer, a server failure) offers "Plan again".
 */
export function DecideNote({
  id,
  reason,
  preview,
}: {
  id: string;
  reason: string | null;
  preview: EffectsPreviewState;
}) {
  return (
    <div className={reason ? "mt-1.5 flex flex-wrap items-center gap-2 text-sm text-med" : undefined}>
      <p id={id} role="status" className={reason ? "m-0" : "sr-only"}>
        {reason ?? ""}
      </p>
      {reason && preview.state === "failed" && (
        <Tooltip tip="Asks the server to plan the effects of your current input now.">
          <button type="button" onClick={preview.planAgain} className={buttonClass("plain", true)}>
            Plan again
          </button>
        </Tooltip>
      )}
    </div>
  );
}
