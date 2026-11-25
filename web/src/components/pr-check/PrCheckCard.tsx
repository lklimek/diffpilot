import { useId, type ReactNode } from "react";
import type { AnalysisEstimate, PrSize } from "../../api/generated/types";
import { estimateText, grouped } from "../pr-form/SizeWarning";

/** The size and findings from the read-only check on the PR page. */
export function PrCheckCard({
  size,
  estimate,
  children,
  actions,
}: {
  size?: PrSize;
  estimate?: AnalysisEstimate;
  children: ReactNode;
  actions?: (describedBy: string) => ReactNode;
}) {
  const id = useId();
  return (
    <div
      className="grid min-w-0 gap-2.5"
      onKeyDownCapture={(event) => {
        // A held key from opening the PR must never start an analysis.
        if (event.repeat && (event.key === "Enter" || event.key === " ")) event.preventDefault();
      }}
    >
      <div id={id} className="grid gap-2.5">
        {size && (
          <p className="text-sm">
            {grouped(size.changed_lines)} changed lines in {grouped(size.changed_files)} files.
            {estimate && ` ${estimateText(estimate)}`}
          </p>
        )}
        {children}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions(id)}</div>}
    </div>
  );
}
