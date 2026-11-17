import { Tooltip } from "../tooltip/Tooltip";

interface Props {
  done: number;
  total: number;
  background: boolean;
  busy: boolean;
  progressHref: string;
  error?: string;
  onBackground: (enabled: boolean) => void;
}

/** Analysis work remains separate from the reviewer's ticks. */
export function FlowWork({ done, total, background, busy, progressHref, error, onBackground }: Props) {
  return (
    <section
      aria-label="Analysis work"
      className="mb-3 rounded border border-border bg-surface px-3 py-2 text-sm"
    >
      <p>
        {done} of {total} flows analysed. Pending flows have not been assessed.
      </p>
      <Tooltip tip="When off, only requested flows run. The running job finishes first.">
        <label className="inline-flex items-center gap-2">
          <input
            type="checkbox"
            checked={background}
            aria-disabled={busy || undefined}
            onChange={(e) => !busy && onBackground(e.target.checked)}
          />
          Analyse the remaining flows in the background
        </label>
      </Tooltip>
      <p>
        <Tooltip tip="Shows analysis timing and the controls to pause or cancel further work.">
          <a href={progressHref}>Analysis progress, pause and cancel</a>
        </Tooltip>
      </p>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
