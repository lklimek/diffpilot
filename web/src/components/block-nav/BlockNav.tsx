import { Kbd } from "../help/Kbd";
import { Tooltip } from "../tooltip/Tooltip";

interface BlockNavProps {
  /** Title of the previous block; absent at the start of the tour. */
  previous?: string;
  /** Title of the next block; absent at the end of the tour. */
  next?: string;
  /** Next also ticks the current block as reviewed (a step). */
  marksReviewed?: boolean;
  onPrevious: () => void;
  onNext: () => void;
  /** Shows the k / j keys on the buttons (the Files view). */
  keys?: boolean;
}

const BUTTON =
  "max-w-1/2 cursor-pointer truncate rounded-md border px-3 py-1.5 aria-disabled:cursor-not-allowed aria-disabled:opacity-55";

/** Previous / Next under the block. */
export function BlockNav({ previous, next, marksReviewed, onPrevious, onNext, keys = false }: BlockNavProps) {
  return (
    <div className="mt-3.5 flex items-center gap-2.5 border-t border-border pt-2.5">
      <Tooltip
        tip={
          previous === undefined ? "This is the first block; there is none before it." : "Previous block (k)"
        }
        className="max-w-1/2"
      >
        <button
          type="button"
          aria-disabled={previous === undefined || undefined}
          onClick={() => previous !== undefined && onPrevious()}
          className={`${BUTTON} max-w-full border-border bg-surface text-text hover:border-accent`}
        >
          {keys && <Kbd decorative>k</Kbd>}{" "}
          {previous === undefined ? "← Previous" : `← Previous: ${previous}`}
        </button>
      </Tooltip>
      <span className="flex-1" />
      <Tooltip
        tip={
          next === undefined
            ? "This is the last block; there is none after it."
            : marksReviewed
              ? "Marks this step reviewed and goes to the next block (j)."
              : "Next block (j)"
        }
        className="max-w-1/2"
      >
        <button
          type="button"
          aria-disabled={next === undefined || undefined}
          onClick={() => next !== undefined && onNext()}
          className={`${BUTTON} max-w-full border-accent bg-accent font-semibold text-on-accent`}
        >
          {next === undefined ? "Next →" : `Next: ${next} →`} {keys && <Kbd decorative>j</Kbd>}
        </button>
      </Tooltip>
    </div>
  );
}
