import type { ReactNode } from "react";
import { Popover } from "../popover/Popover";

type Fold =
  { from: number; to: number; onExpand: () => void } | { direction: "above" | "below"; children: ReactNode };

/** Range folds in Files and around a Flows step. */
export function ContextFold(fold: Fold) {
  return (
    <div
      data-unchanged-fold={!("direction" in fold) || undefined}
      data-context-fold={"direction" in fold ? fold.direction : undefined}
      className="sticky left-0 flex flex-wrap items-center gap-x-2 rounded-md border border-dashed border-border bg-surface-2 px-2 py-0.5 font-sans text-[12px] text-muted"
    >
      <span aria-hidden>⋯</span>
      {"direction" in fold ? fold.children : <UnchangedLines {...fold} />}
    </div>
  );
}

function UnchangedLines({ from, to, onExpand }: Extract<Fold, { from: number }>) {
  const n = to - from + 1;
  const range = from === to ? from : `${from}–${to}`;
  const lines = `${n} unchanged ${n === 1 ? "line" : "lines"} (${range})`;
  return (
    <>
      <span>{lines}</span>
      <Popover
        label={`Show ${lines}`}
        hint="Shows these unchanged lines here."
        onActivate={onExpand}
        content={() => (
          <span className="block">
            Shows lines {range} here in the code. Loaded from the diff by range; nothing is sent to the LLM.
          </span>
        )}
      >
        <button
          type="button"
          aria-label={`Show ${lines}`}
          className="min-h-6 cursor-pointer rounded border border-border bg-surface px-2 text-text hover:border-accent"
        >
          Show
        </button>
      </Popover>
    </>
  );
}

export const CONTEXT_ACTION = "min-h-6 cursor-pointer text-left text-accent underline";
