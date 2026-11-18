import { useEffect, useRef, type ReactNode } from "react";
import type { PrSummary } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

interface PrCheckCardProps {
  /** The pull request found; null when the check failed (the children say why). */
  pr: PrSummary | null;
  /** What the check found: size, estimate, an existing analysis, or the error. */
  children: ReactNode;
  /** Buttons or links; the one marked `data-primary` takes the focus when the card appears. */
  actions?: ReactNode;
  /** Clears the check. */
  onCancel: () => void;
}

/** Result of Home's Check, apart from the field: what was found and the explicit next step. */
export function PrCheckCard({ pr, children, actions, onCancel }: PrCheckCardProps) {
  const card = useRef<HTMLElement>(null);
  useEffect(() => {
    card.current?.scrollIntoView?.({ block: "nearest" });
    card.current?.querySelector<HTMLElement>("[data-primary]")?.focus();
  }, [pr?.html_url, pr?.head_sha]);
  return (
    <section
      ref={card}
      aria-label="Pull request check"
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
      {children}
      <div className="flex flex-wrap items-center gap-2">
        {actions}
        <Tooltip tip="Clears this check; nothing was started.">
          <button type="button" onClick={onCancel} className={buttonClass("plain")}>
            Cancel
          </button>
        </Tooltip>
      </div>
    </section>
  );
}
