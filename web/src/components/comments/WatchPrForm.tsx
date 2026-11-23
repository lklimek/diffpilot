import { useId, useRef, useState, type FormEvent } from "react";
import type { PrRef, WatchList } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { codeOf } from "../inspector/action";
import { Tooltip } from "../tooltip/Tooltip";

function pullRequest(text: string): PrRef | null {
  try {
    const url = new URL(text.trim());
    if (url.protocol !== "https:" || url.host !== "github.com" || url.username || url.password) return null;
    const match = /^\/([a-z\d-]+)\/([a-z\d_.-]+)\/pull\/([1-9]\d*)(?:\/files)?\/?$/i.exec(url.pathname);
    if (!match || !Number.isSafeInteger(Number(match[3]))) return null;
    return { repo: { owner: match[1]!, name: match[2]! }, number: Number(match[3]) };
  } catch {
    return null;
  }
}

function failure(error: unknown, limit: number): string {
  if (codeOf(error) === "private_repository")
    return "Private repositories are not supported. Watch reads public repositories only.";
  if (codeOf(error) === "watch_limit_reached")
    return `Watch limit reached (${limit} pull requests). Unwatch one before adding another.`;
  if (error instanceof Error) return `${error.message} Try again.`;
  return "Could not watch this pull request. Try again.";
}

/** Add a public PR to Watch using a pasted link; requests belong to the callback. */
export function WatchPrForm({ list, onWatch }: { list: WatchList; onWatch: (pr: PrRef) => Promise<void> }) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState("");
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setSuccess("");
    const pr = pullRequest(link);
    if (!pr) {
      setError("Enter a GitHub pull request link: https://github.com/owner/repo/pull/123.");
      input.current?.focus();
      return;
    }
    const same = (other: PrRef) =>
      other.number === pr.number &&
      other.repo.owner.toLowerCase() === pr.repo.owner.toLowerCase() &&
      other.repo.name.toLowerCase() === pr.repo.name.toLowerCase();
    if (list.watches.some((watch) => watch.state !== "ended" && same(watch.pr))) {
      setError("This pull request is already watched.");
      input.current?.focus();
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await onWatch(pr);
      setLink("");
      setSuccess(`Watching ${pr.repo.owner}/${pr.repo.name} #${pr.number}.`);
      input.current?.focus();
    } catch (error) {
      setError(failure(error, list.limit));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      onSubmit={(event) => void submit(event)}
      aria-label="Watch a pull request"
      aria-busy={busy}
      className="grid gap-2 rounded-lg border border-border bg-surface p-3"
    >
      <label htmlFor={id} className="text-sm font-semibold">
        GitHub pull request link
      </label>
      <p id={`${id}-help`} className="m-0 text-xs text-muted">
        Public repositories only. Watch reads the comments and checks for new ones automatically; new comments
        may be assessed with the LLM.
      </p>
      <div className="flex flex-wrap gap-2">
        <Tooltip tip="Paste the HTTPS link of a public GitHub pull request to watch.">
          <input
            ref={input}
            id={id}
            type="text"
            inputMode="url"
            value={link}
            readOnly={busy}
            aria-invalid={error ? true : undefined}
            aria-describedby={`${id}-help${error ? ` ${id}-error` : ""}`}
            onChange={(event) => {
              setLink(event.target.value);
              setError(null);
              setSuccess("");
            }}
            placeholder="https://github.com/owner/repo/pull/123"
            className="w-full min-w-0 rounded border border-muted bg-surface px-2 py-1 text-text sm:w-96"
          />
        </Tooltip>
        <Tooltip
          tip={
            busy
              ? "Adding this pull request to Watched PRs. Wait for it to finish."
              : "Adds this public pull request to Watched PRs. Watch reads its comments; no code analysis is started."
          }
        >
          <button type="submit" aria-disabled={busy || undefined} className={buttonClass("plain")}>
            {busy ? "Adding…" : "Watch pull request"}
          </button>
        </Tooltip>
      </div>
      {error && (
        <p id={`${id}-error`} role="alert" className="m-0 text-sm text-high">
          {error}
        </p>
      )}
      <p role="status" className="m-0 text-sm">
        {success}
      </p>
    </form>
  );
}
