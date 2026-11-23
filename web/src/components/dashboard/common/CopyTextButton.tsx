import { useEffect, useRef, useState } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";
import { writeClipboard } from "./copyList";

/** How long the result stays shown, in ms. */
const SHOWN_MS = 5000;

/**
 * A button that copies `text` as plain text only (no HTML part), through the same clipboard helper as Copy list,
 * and says whether it worked (visible and announced). `what` names the text in the result ("the prompt").
 */
export function CopyTextButton({
  text,
  label,
  tip,
  what,
}: {
  text: string;
  label: string;
  tip: string;
  what: string;
}) {
  const [result, setResult] = useState("");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    window.clearTimeout(timer.current);
    // Empty first, so a repeated result is announced again.
    setResult("");
    setResult(
      (await writeClipboard({ text }))
        ? `Copied ${what}.`
        : `Copying did not work in this browser. Select ${what} and copy it yourself.`,
    );
    timer.current = window.setTimeout(() => setResult(""), SHOWN_MS);
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Tooltip tip={tip}>
        <button type="button" className={`${buttonClass("plain", true)} min-h-6`} onClick={() => void copy()}>
          {label}
        </button>
      </Tooltip>
      <span role="status" className="text-xs text-muted">
        {result}
      </span>
    </div>
  );
}
