import { useId, useRef, useState } from "react";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

export function PrUrlForm({
  initialUrl = "",
  initialError = false,
  onOpen,
}: {
  initialUrl?: string;
  initialError?: boolean;
  onOpen: (value: string) => boolean;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(initialUrl);
  const [invalid, setInvalid] = useState(initialError);

  return (
    <form
      aria-labelledby={`${id}-heading`}
      className="min-w-0 rounded-lg border border-border bg-surface p-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (!onOpen(value)) {
          setInvalid(true);
          input.current?.focus();
        }
      }}
    >
      <h2 id={`${id}-heading`} className="mb-2 text-base font-semibold">
        Open a pull request
      </h2>
      <div className="flex flex-wrap items-start gap-2">
        <label htmlFor={id} className="sr-only">
          GitHub pull request URL
        </label>
        <input
          id={id}
          ref={input}
          type="text"
          inputMode="url"
          value={value}
          aria-invalid={invalid || undefined}
          aria-describedby={invalid ? `${id}-error` : undefined}
          onChange={(event) => {
            setValue(event.target.value);
            setInvalid(false);
          }}
          placeholder="https://github.com/owner/repo/pull/123 or owner/repo#123"
          className="min-h-8 w-full min-w-0 rounded border border-muted bg-surface px-2 py-1 sm:w-auto sm:flex-1"
        />
        <Tooltip tip="Opens the pull request page, where you can start analysis or process comments.">
          <button type="submit" className={buttonClass("primary")}>
            Open
          </button>
        </Tooltip>
      </div>
      {invalid && (
        <p id={`${id}-error`} role="alert" className="mt-2 text-sm text-high [overflow-wrap:anywhere]">
          Enter a GitHub pull request link like https://github.com/owner/repo/pull/123 or owner/repo#123.
        </p>
      )}
    </form>
  );
}
