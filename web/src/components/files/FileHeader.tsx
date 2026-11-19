import type { Ref } from "react";
import type { FileViewedState, PrFile } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { Kbd } from "../help/Kbd";
import { busyTip } from "../button/button";
import { fileDiffUrl, type PrLinkBase } from "../github/links";
import { GithubLink } from "../inspector/text";
import { Tooltip } from "../tooltip/Tooltip";

/** The Viewed toggle's state: what GitHub said, a save in flight, why it cannot change, the last refusal. */
export interface ViewedControl {
  /** The mark read from GitHub; null when it was not read. */
  state: FileViewedState | null;
  /** Waiting for GitHub to confirm a change. */
  saving: boolean;
  /** Why the mark cannot be changed here (head moved, not read); null when it can. */
  disabledReason: string | null;
  /** The last change that failed, with the server's message (written for the reader). */
  error: { message: string; retry: () => void } | null;
  /** Marks or unmarks the file; the screen opens the next file once GitHub confirmed a mark. */
  onToggle: () => void;
}

interface FileHeaderProps {
  file: PrFile;
  pr: PrLinkBase;
  viewed: ViewedControl;
  /** "Show old code": the classic diff under every hunk; no button without it. */
  oldCode?: { shown: boolean; onToggle: () => void };
  /** The heading, which takes the focus when the file opens after a Viewed mark. */
  headingRef?: Ref<HTMLHeadingElement>;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold whitespace-nowrap text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg aria-disabled:cursor-not-allowed aria-disabled:opacity-60";

const STATUS: Record<PrFile["status"], string> = {
  added: "added",
  modified: "modified",
  deleted: "deleted",
  renamed: "renamed",
};

const VIEWED_TIP =
  "Your GitHub 'Viewed' mark for this file in this pull request (v). Read from GitHub when the review opens, saved to GitHub when you change it; marking it opens the next file. Separate from step ticks.";

/** Header of the open file (brief §4.5): path, change, counts, GitHub link, Show old code, Viewed. */
export function FileHeader({ file, pr, viewed, oldCode, headingRef }: FileHeaderProps) {
  const github = fileDiffUrl(pr, file.path);
  const blocked = viewed.saving || viewed.disabledReason !== null;
  const on = viewed.state === "viewed";
  return (
    <header className="grid gap-1.5 rounded-lg border border-border bg-surface px-3 py-2">
      <div className="flex flex-wrap items-center gap-2">
        <h2 ref={headingRef} tabIndex={-1} className="m-0 min-w-0 font-mono text-[14px] font-bold break-all">
          {file.path}
        </h2>
        <Tooltip tip="How git reports this file in the diff (deterministic).">
          <span
            tabIndex={0}
            className="cursor-help rounded border border-mv-bd bg-surface-2 px-1.5 text-[11px] leading-[18px] font-semibold"
          >
            {STATUS[file.status]}
          </span>
        </Tooltip>
        {file.group.kind === "not_analysed" && <ProvenanceBadge badge={file.group.badge} />}
        {file.old_path && <span className="text-[12px] text-muted">from {file.old_path}</span>}
        <span className="font-mono text-[12px]">
          <span className="text-add">+{file.added}</span> <span className="text-high">−{file.removed}</span>
        </span>
        <GithubLink
          url={github}
          tip="Open this file in the pull request diff on GitHub (new tab). GitHub shows the latest commits, which may differ from the analysed ones."
        >
          GitHub
        </GithubLink>
        <span className="ml-auto flex flex-wrap gap-2">
          {oldCode && (
            <Tooltip tip="Show or hide the classic diff under every hunk of this file (d)">
              <button
                type="button"
                aria-pressed={oldCode.shown}
                onClick={oldCode.onToggle}
                className={BUTTON}
              >
                Show old code <Kbd decorative>d</Kbd>
              </button>
            </Tooltip>
          )}
          <Tooltip tip={busyTip(viewed.disabledReason ?? VIEWED_TIP, viewed.saving)}>
            <button
              type="button"
              aria-pressed={on}
              aria-disabled={blocked || undefined}
              onClick={() => !blocked && viewed.onToggle()}
              className={BUTTON}
            >
              {viewed.saving ? (
                "Saving to GitHub…"
              ) : (
                <>
                  <span aria-hidden>{on ? "☑" : "☐"} </span>Viewed <Kbd decorative>v</Kbd>
                </>
              )}
            </button>
          </Tooltip>
        </span>
      </div>
      {viewed.state === "dismissed" && (
        <p className="m-0 text-[12.5px] text-med">
          Changed since you marked it viewed: GitHub cleared the mark when new commits changed this file. Mark
          it again after reading.
        </p>
      )}
      {viewed.disabledReason && <p className="m-0 text-[12.5px] text-muted">{viewed.disabledReason}</p>}
      {viewed.error && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-2 rounded-md border border-l-4 border-border border-l-high-bd bg-surface px-3 py-1 text-[12.5px]"
        >
          <p className="m-0 flex-1">{viewed.error.message}</p>
          <Tooltip tip="Sends the same change to GitHub again.">
            <button type="button" onClick={viewed.error.retry} className={BUTTON}>
              Retry
            </button>
          </Tooltip>
          <GithubLink
            url={github}
            tip="Open this file in the pull request on GitHub to mark it there (new tab)."
          >
            Mark it on GitHub
          </GithubLink>
        </div>
      )}
    </header>
  );
}
