import { SizeWarning } from "./SizeWarning";
import type { SizeWarning as Warning } from "../../api/generated/types";
import { useId, useState } from "react";
import { busyTip, buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

interface PrUrlFormProps {
  /** Value the field starts with. */
  initialUrl?: string;
  /** A request is running: Analyze is unavailable (aria-disabled, its tip says why). */
  busy: boolean;
  sizeWarning?: { url: string; warning: Warning };
  /** Called with the trimmed URL. */
  onSubmit: (url: string) => void;
}

/** The one input of Home: GitHub PR URL with an example, and Analyze. */
export function PrUrlForm({ initialUrl = "", busy, onSubmit, sizeWarning }: PrUrlFormProps) {
  const id = useId();
  const [url, setUrl] = useState(initialUrl);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!busy) onSubmit(url.trim());
      }}
    >
      <label htmlFor={id} className="mb-1 block text-xs font-semibold">
        GitHub pull request URL
      </label>
      <div className="flex gap-2">
        <input
          id={id}
          type="url"
          required
          autoComplete="off"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          aria-describedby={`${id}-help`}
          className="min-w-0 flex-1 rounded-md border border-muted bg-surface px-2.5 py-1.5 font-mono"
        />
        <Tooltip
          tip={busyTip(
            "Opens the pull request: an existing analysis is resumed, otherwise a new one starts.",
            busy,
          )}
        >
          <button type="submit" aria-disabled={busy || undefined} className={buttonClass("primary")}>
            Analyze
          </button>
        </Tooltip>
      </div>
      {sizeWarning?.url === url.trim() && <SizeWarning warning={sizeWarning.warning} />}
      <p id={`${id}-help`} className="mt-1 text-xs text-muted">
        Example: https://github.com/owner/repo/pull/123
      </p>
    </form>
  );
}
