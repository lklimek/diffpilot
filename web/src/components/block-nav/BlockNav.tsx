import { Tooltip } from "../tooltip/Tooltip";

interface BlockNavProps {
  /** Title of the previous block; absent at the start of the tour. */
  previous?: string;
  /** Title of the next block; absent at the end of the tour. */
  next?: string;
  onPrevious: () => void;
  onNext: () => void;
}

const BUTTON =
  "max-w-1/2 cursor-pointer truncate rounded-md border px-3 py-1.5 disabled:cursor-not-allowed disabled:opacity-55";

/** Previous / Next under the block. */
export function BlockNav({ previous, next, onPrevious, onNext }: BlockNavProps) {
  return (
    <div className="mt-3.5 flex items-center gap-2.5 border-t border-border pt-2.5">
      <Tooltip tip="Previous block (k)" className="max-w-1/2">
        <button
          type="button"
          disabled={previous === undefined}
          onClick={onPrevious}
          className={`${BUTTON} max-w-full border-border bg-surface text-text hover:border-accent`}
        >
          {previous === undefined ? "← Previous" : `← Previous: ${previous}`}
        </button>
      </Tooltip>
      <span className="flex-1" />
      <Tooltip tip="Next block (j)" className="max-w-1/2">
        <button
          type="button"
          disabled={next === undefined}
          onClick={onNext}
          className={`${BUTTON} max-w-full border-accent bg-accent font-semibold text-on-accent`}
        >
          {next === undefined ? "Next →" : `Next: ${next} →`}
        </button>
      </Tooltip>
    </div>
  );
}
