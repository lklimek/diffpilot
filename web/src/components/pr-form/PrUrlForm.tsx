import { useId, useState, type Ref } from "react";
import { busyTip, buttonClass } from "../button/button";
import { assignRef } from "../inspector/action";
import { Tooltip } from "../tooltip/Tooltip";

interface PrUrlFormProps {
  /** Value the field starts with. */
  initialUrl?: string;
  /** A request is running: Check is unavailable (aria-disabled, its tip says why). */
  busy: boolean;
  /** The check of the URL is the running request: shown on the field and the button. */
  checking?: boolean;
  /** The URL field, e.g. to return the focus to it. */
  inputRef?: Ref<HTMLInputElement>;
  /** Called with the trimmed URL. */
  onSubmit: (url: string) => void;
  /** Called with the trimmed URL on every edit. */
  onChange?: (url: string) => void;
}

/** The one input of Home: GitHub PR URL with an example, and Check (a lookup; it starts nothing). */
export function PrUrlForm({
  initialUrl = "",
  busy,
  checking = false,
  inputRef,
  onSubmit,
  onChange,
}: PrUrlFormProps) {
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
          ref={(el) => assignRef(inputRef, el)}
          id={id}
          type="url"
          required
          autoComplete="off"
          value={url}
          aria-busy={checking || undefined}
          onChange={(event) => {
            setUrl(event.target.value);
            onChange?.(event.target.value.trim());
          }}
          aria-describedby={`${id}-help`}
          className={`min-w-0 flex-1 rounded-md border bg-surface px-2.5 py-1.5 font-mono ${
            checking ? "border-accent motion-safe:animate-pulse" : "border-muted"
          }`}
        />
        <Tooltip
          tip={busyTip(
            "Looks the pull request up: any existing analysis, and a size warning for a large one. Nothing starts until you press Start analysis.",
            busy,
          )}
        >
          <button type="submit" aria-disabled={busy || undefined} className={buttonClass("primary")}>
            {checking ? "Checking…" : "Check"}
          </button>
        </Tooltip>
      </div>
      <p id={`${id}-help`} className="mt-1 text-xs text-muted">
        Example: https://github.com/owner/repo/pull/123
      </p>
    </form>
  );
}
