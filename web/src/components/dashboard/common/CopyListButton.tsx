import { useEffect, useRef, useState } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";
import { copyText, writeClipboard, type CopyLine } from "./copyList";

/** How long the result stays shown, in ms. */
const SHOWN_MS = 5000;

/**
 * "Copy list": copies `lines` as linked lines to paste into Slack, and says whether it worked (visible and
 * announced). Nothing when the list is empty.
 */
export function CopyListButton({ lines, what }: { lines: CopyLine[]; what: string }) {
  const [result, setResult] = useState<string>("");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  if (lines.length === 0) return null;
  const copy = async () => {
    window.clearTimeout(timer.current);
    // Empty first, so a repeated result is announced again.
    setResult("");
    const copied = await writeClipboard(copyText(lines));
    setResult(
      copied
        ? `Copied ${lines.length} ${lines.length === 1 ? "line" : "lines"}.`
        : "Copying did not work in this browser.",
    );
    timer.current = window.setTimeout(() => setResult(""), SHOWN_MS);
  };
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2">
      <Tooltip tip="Copies one line per PR with its GitHub link, to paste into Slack.">
        <button
          type="button"
          aria-label={`Copy list of ${what}`}
          className={`${buttonClass("plain", true)} min-h-6`}
          onClick={() => void copy()}
        >
          Copy list
        </button>
      </Tooltip>
      <span role="status" className="text-xs text-muted">
        {result}
      </span>
    </div>
  );
}
