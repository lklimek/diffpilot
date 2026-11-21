import { Popover } from "../popover/Popover";

/**
 * Unchanged lines folded between the changes of a file ("⋯ 80 unchanged lines (1–80)"): one click, Enter or
 * Space shows them; hover or focus only explains (brief §4.3 folded rows). Loaded by range, never sent to the LLM.
 */
export function UnchangedFold({
  from,
  to,
  onExpand,
}: {
  from: number;
  /** Last folded line. */
  to: number;
  onExpand: () => void;
}) {
  const n = to - from + 1;
  const lines = `${n} unchanged ${n === 1 ? "line" : "lines"} (${from === to ? from : `${from}–${to}`})`;
  return (
    <div
      data-unchanged-fold
      className="flex items-center gap-2 rounded-md border border-dashed border-border bg-surface-2 px-2 py-0.5 font-sans text-[12px] text-muted"
    >
      <span aria-hidden>⋯</span>
      <span>{lines}</span>
      <Popover
        label={`Show ${lines}`}
        hint="Shows these unchanged lines here."
        onActivate={onExpand}
        content={() => (
          <span className="block">
            Shows lines {from === to ? from : `${from}–${to}`} here in the code. Loaded from the diff by
            range; nothing is sent to the LLM.
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
    </div>
  );
}
