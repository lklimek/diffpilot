import { Kbd } from "../../help/Kbd";
import { Tooltip } from "../../tooltip/Tooltip";

const BUTTON =
  "inline-flex min-h-6 cursor-pointer items-center gap-1 rounded-md border border-border bg-surface px-2 text-[12.5px] whitespace-nowrap text-text hover:border-accent";

/** "Ask about this step `a`" and its kin: opens the Ask chat with that subject; nothing is sent (proposal AC9). */
export function AskButton({ what, onAsk }: { what: string; onAsk: () => void }) {
  return (
    <Tooltip tip={`Opens your Ask chat about ${what} (a). Nothing is sent until you press Ask.`}>
      <button type="button" onClick={onAsk} className={BUTTON}>
        Ask about {what} <Kbd decorative>a</Kbd>
      </button>
    </Tooltip>
  );
}
