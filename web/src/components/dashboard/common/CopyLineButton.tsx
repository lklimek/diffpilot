import { useEffect, useRef, useState } from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { copyText, writeClipboard, type CopyLine } from "./copyList";
import { CopyIcon, SlackIcon } from "./icons";

/** How long the result stays shown, in ms. */
const SHOWN_MS = 3000;

/**
 * Copy icon of one list row: copies the row's linked line (as Copy list does) to paste into Slack and says whether
 * it worked (announced; a ✓ or ✗ next to the icon). `slack`: the Slack icon, for a line with Slack names (K19).
 */
export function CopyLineButton({ line, id, slack = false }: { line: CopyLine; id: string; slack?: boolean }) {
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
      <Tooltip
        tip={
          slack
            ? `Copies the line of ${id} with its GitHub link and the Slack names of Settings › GitHub sync for GitHub logins; a login without one stays @login.`
            : `Copies the line of ${id} with its GitHub link, to paste into Slack.`
        }
      >
        <button
          type="button"
          aria-label={slack ? `Copy line of ${id} for Slack` : `Copy line of ${id}`}
          onClick={(event) => {
            event.stopPropagation();
            void copy();
          }}
          // Glyph-height mark, 24 px target (WCAG 2.5.8), like the GitHub link next to it.
          className="-my-1 inline-flex min-h-6 min-w-6 shrink-0 cursor-pointer items-center justify-center rounded border-0 bg-transparent p-0 text-accent hover:bg-accent-bg"
        >
          {slack ? <SlackIcon /> : <CopyIcon />}
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
