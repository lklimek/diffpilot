import { useId, useRef, useState } from "react";
import type { LlmCallOutcome, RankingStatus } from "../../api/generated/types";
import type { LlmActivity } from "../../api/provisional/llmActivity";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText, dateTimeText } from "../format/time";
import { useOutsidePress } from "../popover/useOutsidePress";
import { Tooltip } from "../tooltip/Tooltip";
import { LlmActivityList } from "./LlmActivityList";

const ACTION =
  "flex min-h-7 w-full cursor-pointer items-center justify-between gap-2 rounded px-2.5 py-1 text-left text-text no-underline hover:bg-accent-bg focus-visible:bg-accent-bg";

export interface UserMenuProps {
  login: string;
  settingsHref: string;
  usageHref: string;
  now: Date;
  activity: LlmActivity | null | undefined;
  legacyRanking?: RankingStatus | null;
  pausedUntil?: string | null;
  lastCall?: LlmCallOutcome | null;
  onNavigate?: Navigate;
  onHelp?: () => void;
  onSignOut: () => void;
  signingOut?: boolean;
  signOutError?: string;
}

/** A non-modal disclosure with account actions and LLM status. */
export function UserMenu({
  login,
  settingsHref,
  usageHref,
  now,
  activity,
  legacyRanking,
  pausedUntil = activity?.paused_until ?? null,
  lastCall = activity?.last_call ?? null,
  onNavigate,
  onHelp,
  onSignOut,
  signingOut = false,
  signOutError,
}: UserMenuProps) {
  const id = useId();
  const wrapper = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  useOutsidePress(open, wrapper, () => setOpen(false));

  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  const paused = pausedUntil && Date.parse(pausedUntil) > now.getTime() ? pausedUntil : null;
  const failed = lastCall?.status.state === "not_working";
  const attention = paused ? "paused" : failed ? "failed" : null;
  const panelName = `${login}: account and LLM activity`;
  const name = paused
    ? `${login} — LLM paused until ${clockText(paused, now)}`
    : failed
      ? `${login} — LLM not working`
      : panelName;
  const follow =
    onNavigate &&
    ((href: string) => {
      setOpen(false);
      onNavigate(href);
    });

  return (
    <div
      ref={wrapper}
      className="relative flex flex-none items-center"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !open) return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}
      onBlur={(event) => {
        if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <Tooltip tip={attention ? name : `Signed in as ${login} (GitHub)`}>
        <button
          ref={trigger}
          type="button"
          aria-label={name}
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls={open ? id : undefined}
          onClick={() => setOpen(!open)}
          className="inline-flex min-h-7 min-w-6 cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-[13px] text-text hover:bg-surface-2 aria-expanded:bg-surface-2"
        >
          <span
            aria-hidden
            className="grid size-5 place-items-center rounded-full bg-accent-bg text-xs font-semibold text-accent"
          >
            {login.slice(0, 1)}
          </span>
          <span aria-hidden className="user-menu-login hidden max-w-32 truncate sm:inline">
            {login}
          </span>
          {attention && (
            <span
              aria-hidden
              data-llm-attention={attention}
              className={`grid size-3.5 place-items-center rounded-full text-[10px] font-bold leading-none ${
                paused ? "bg-med text-med-bg" : "bg-high text-high-bg"
              }`}
            >
              {paused ? "⏸" : "✕"}
            </span>
          )}
          <span aria-hidden className="text-muted">
            ▾
          </span>
        </button>
      </Tooltip>
      {failed && (
        <span role="status" className="sr-only">
          Last LLM call failed at {dateTimeText(lastCall.at)}.
        </span>
      )}
      {open && (
        <div
          id={id}
          role="dialog"
          aria-label={panelName}
          className="absolute right-0 top-full z-50 mt-1 grid max-h-[calc(100dvh-4rem)] w-88 max-w-[calc(100vw-2rem)] gap-1 overflow-y-auto overscroll-contain rounded-lg border border-border bg-surface p-1 text-left text-[13px] whitespace-normal text-text shadow-tip [overflow-wrap:anywhere]"
        >
          <p className="px-2.5 py-1 text-xs text-muted">Signed in as {login} (GitHub)</p>
          <LlmActivityList
            activity={activity}
            legacyRanking={legacyRanking}
            lastCall={lastCall}
            pausedUntil={paused}
            now={now}
            usageHref={usageHref}
            onNavigate={follow}
          />
          <a href={settingsHref} onClick={navigateOnClick(settingsHref, follow)} className={ACTION}>
            Settings
          </a>
          {onHelp && (
            <Tooltip tip="Show keyboard shortcuts for this page.">
              <button
                type="button"
                aria-keyshortcuts="?"
                onClick={() => {
                  close();
                  onHelp();
                }}
                className={ACTION}
              >
                Keyboard shortcuts{" "}
                <kbd aria-hidden className="rounded border border-border px-1 text-xs">
                  ?
                </kbd>
              </button>
            </Tooltip>
          )}
          <Tooltip tip="End your diffpilot session in this browser.">
            <button
              type="button"
              aria-disabled={signingOut || undefined}
              onClick={() => !signingOut && onSignOut()}
              className={ACTION}
            >
              {signingOut ? "Signing out…" : "Sign out"}
            </button>
          </Tooltip>
          {signOutError && (
            <p role="alert" className="px-2.5 text-xs text-high">
              Not signed out: {signOutError}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
