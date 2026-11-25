import { useEffect, useRef, useState } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";
import { copyText, writeClipboard, type CopyLine } from "./copyList";

/** How long the result stays shown, in ms. */
const SHOWN_MS = 5000;

/**
 * "Copy list": copies `lines` as linked lines to paste into Slack, and says whether it worked (visible and
 * announced). `slackLines`: the same lines with Slack names, copied by "Copy for Slack" next to it (K19). Nothing
 * when the list is empty.
 */
export function CopyListButton({
  lines,
  slackLines,
  what,
}: {
  lines: CopyLine[];
  slackLines?: CopyLine[];
  what: string;
}) {
  const [result, setResult] = useState<string>("");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  if (lines.length === 0) return null;
  const copy = async (rows: CopyLine[]) => {
    window.clearTimeout(timer.current);
    // Empty first, so a repeated result is announced again.
    setResult("");
    const copied = await writeClipboard(copyText(rows));
    setResult(
      copied
        ? `Copied ${rows.length} ${rows.length === 1 ? "line" : "lines"}.`
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
          onClick={() => void copy(lines)}
        >
          Copy list
        </button>
      </Tooltip>
      {slackLines && (
        <Tooltip tip="Copies the same lines with the Slack names of Settings › GitHub sync for GitHub logins; a login without one stays @login.">
          <button
            type="button"
            aria-label={`Copy for Slack: list of ${what}`}
            className={`${buttonClass("plain", true)} min-h-6`}
            onClick={() => void copy(slackLines)}
          >
            Copy for Slack
          </button>
        </Tooltip>
      )}
      <span role="status" className="text-xs text-muted">
        {result}
      </span>
    </div>
  );
}
