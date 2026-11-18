import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import type { AnalysisEstimate, PrSize, PrSummary } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { estimateText, grouped } from "../pr-form/SizeWarning";
import { Tooltip } from "../tooltip/Tooltip";

/** Keys right after the card appears belong to what came before (the Enter that ran Check), not to the card. */
export const SETTLE_MS = 400;

interface PrCheckCardProps {
  /** The pull request found; null when the check failed (the children say why). */
  pr: PrSummary | null;
  /** Size facts, when the check returned them. */
  size?: PrSize;
  /** The pre-start estimate (plan and PR context), shown with the size. */
  estimate?: AnalysisEstimate;
  /** What the check found: warnings, an existing analysis, or the error. */
  children: ReactNode;
  /**
   * Buttons or links, given the id of the card's findings for `aria-describedby`; the one marked
   * `data-primary` takes the focus when the card appears.
   */
  actions?: (describedBy: string) => ReactNode;
  /** Clears the check. */
  onCancel: () => void;
}

/** Result of Home's Check, apart from the field: what was found and the explicit next step. */
export function PrCheckCard({ pr, size, estimate, children, actions, onCancel }: PrCheckCardProps) {
  const card = useRef<HTMLElement>(null);
  const bodyId = useId();
  const shownAt = useRef(0);
  useEffect(() => {
    shownAt.current = performance.now();
    card.current?.scrollIntoView?.({ block: "nearest" });
    card.current?.querySelector<HTMLElement>("[data-primary]")?.focus();
  }, [pr?.html_url, pr?.head_sha]);
  // A held Enter (auto-repeat) or one in the moment the card appears never starts anything unseen.
  const guard = (event: KeyboardEvent) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    if (event.repeat || performance.now() - shownAt.current < SETTLE_MS) event.preventDefault();
  };
  return (
    <section
      ref={card}
      aria-label="Pull request check"
      onKeyDownCapture={guard}
      className="grid gap-2.5 rounded-lg border border-border bg-surface px-4 py-3.5 shadow-sm"
    >
      {pr && (
        <header>
          <h2 className="text-base font-semibold">
            <span className="text-muted">
              {pr.repo.owner}/{pr.repo.name} #{pr.number}
            </span>{" "}
            {pr.title}
          </h2>
          <p className="text-xs text-muted">by {pr.author}</p>
        </header>
      )}
      <div id={bodyId} className="grid gap-2.5">
        {size && (
          <p className="text-sm">
            {grouped(size.changed_lines)} changed lines in {grouped(size.changed_files)} files.
            {estimate && ` ${estimateText(estimate)}`}
          </p>
        )}
        {children}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {actions?.(bodyId)}
        <Tooltip tip="Clears this check; nothing was started.">
          <button type="button" onClick={onCancel} className={buttonClass("plain")}>
            Cancel
          </button>
        </Tooltip>
      </div>
    </section>
  );
}
