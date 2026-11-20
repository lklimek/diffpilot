import { useEffect, useRef, useState } from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { copyText, writeClipboard, type CopyLine } from "./copyList";
import { CopyIcon } from "./icons";

/** How long the result stays shown, in ms. */
const SHOWN_MS = 3000;

/**
 * Copy icon of one list row: copies the row's linked line (as Copy list does) to paste into Slack and says whether
 * it worked (announced; a ✓ or ✗ next to the icon).
 */
export function CopyLineButton({ line, id }: { line: CopyLine; id: string }) {
  const [result, setResult] = useState<"" | "copied" | "failed">("");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    window.clearTimeout(timer.current);
    setResult("");
    setResult((await writeClipboard(copyText([line]))) ? "copied" : "failed");
    timer.current = window.setTimeout(() => setResult(""), SHOWN_MS);
  };
  return (
    <span className="inline-flex items-center">
      <Tooltip tip={`Copies the line of ${id} with its GitHub link, to paste into Slack.`}>
        <button
          type="button"
          aria-label={`Copy line of ${id}`}
          onClick={(event) => {
            event.stopPropagation();
            void copy();
          }}
          // Glyph-height mark, 24 px target (WCAG 2.5.8), like the GitHub link next to it.
          className="-my-1 inline-flex min-h-6 min-w-6 shrink-0 cursor-pointer items-center justify-center rounded border-0 bg-transparent p-0 text-accent hover:bg-accent-bg"
        >
          <CopyIcon />
        </button>
      </Tooltip>
      <span role="status" className="text-xs text-muted">
        {result === "copied" ? (
          <>
            <span aria-hidden>✓</span>
            <span className="sr-only">Copied the line of {id}.</span>
          </>
        ) : result === "failed" ? (
          "Copying did not work in this browser."
        ) : null}
      </span>
    </span>
  );
}
