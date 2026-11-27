import { useId } from "react";
import type { CiStatus, GithubRead, PluginNote, PrBadgeView } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { badgeLook } from "../comments/StateBadge";
import { OpenExternal, httpUrl } from "../dashboard/common/OpenExternal";
import { clockText } from "../format/time";
import { useAnchoredPanel } from "../popover/useAnchoredPanel";
import { Tooltip } from "../tooltip/Tooltip";
import { isGithubUrl } from "../github/links";

interface StatusPopoverProps {
  badge: PrBadgeView;
  rounds: number;
  notes?: PluginNote[];
  read: GithubRead;
  now: Date;
  itemId: string;
  ci?: CiStatus | null;
  variant?: "pr" | "comments";
  busy: boolean;
  onRead: () => void;
}

/** The PR state with the evidence and the GitHub read action in a non-modal disclosure. */
export function StatusPopover({
  badge,
  rounds,
  notes = [],
  read,
  now,
  itemId,
  ci,
  variant = "pr",
  busy,
  onRead,
}: StatusPopoverProps) {
  const id = useId();
  const { root, anchor, panel, open, setOpen, dismiss } = useAnchoredPanel<HTMLElement>({ gap: 8 });
  const state = badgeLook(badge);
  const color =
    variant === "comments" && badge.badge === "questions_waiting"
      ? "border-low-bd bg-low-bg text-low"
      : state.look;
  const ciBreakdown = ci && [ci.check_runs, ci.statuses].filter((source) => source !== undefined);
  const ciWords = ciBreakdown?.length
    ? (["passed", "failed", "pending", "skipped", "neutral"] as const)
        .map((outcome) => [outcome, ciBreakdown.reduce((sum, source) => sum + source[outcome], 0)] as const)
        .filter(([, count]) => count > 0)
        .map(([outcome, count]) => `${count} ${outcome}`)
        .join(", ")
    : null;
  return (
    <div ref={root} className="relative inline-flex max-w-full" {...dismiss}>
      <Tooltip tip="Shows the facts behind the pull request status and when GitHub was read.">
        <button
          ref={anchor}
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls={open ? id : undefined}
          data-state-badge={badge.badge ?? "none"}
          className={`inline-flex min-h-6 cursor-pointer items-center gap-1.5 rounded-full border px-2 text-xs font-semibold ${color}`}
          onClick={() => setOpen(!open)}
        >
          <span aria-hidden>{state.icon}</span> <span aria-live="polite">{state.label}</span>{" "}
          <span aria-hidden>▾</span>
        </button>
      </Tooltip>
      {open && (
        <section
          id={id}
          ref={panel}
          role="dialog"
          aria-label="Pull request status"
          className="fixed z-40 grid w-80 max-w-[calc(100vw-1rem)] gap-3 overflow-y-auto overscroll-contain rounded-lg border border-border bg-surface p-3 text-sm wrap-anywhere shadow-tip"
        >
          <div>
            <h2 className="m-0 text-sm font-semibold">diffpilot state</h2>
            <p className="m-0">
              {state.label}
              {badge.facts.length > 0 && ` · ${badge.facts.join(" · ")}`}
            </p>
            {badge.missing.length > 0 && (
              <p className="m-0 text-xs text-muted">Not known: {badge.missing.join(", ")}.</p>
            )}
            {ciWords && <p className="m-0">CI: {ciWords}</p>}
          </div>
          {notes.map((note) => {
            const url = httpUrl(note.url);
            const evidence =
              url &&
              (isGithubUrl(url) ? (
                <OpenExternal href={url} label={`${note.source} data for ${itemId}`} />
              ) : (
                <a
                  href={url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex min-h-6 items-center text-accent underline"
                  aria-label={`Source for ${note.source} (new tab)`}
                >
                  Source
                </a>
              ));
            if (variant === "comments" && note.plugin === "pr-hygiene") {
              const [first, ...remaining] = note.actions ?? [];
              if (!first) return null;
              return (
                <details key={note.plugin} className="group/hygiene min-w-0" data-plugin-note={note.plugin}>
                  <summary className="flex min-h-6 cursor-pointer list-none items-start gap-1">
                    <span aria-hidden className="group-open/hygiene:rotate-90">
                      ▸
                    </span>
                    <span className="shrink-0 font-semibold">{note.source}</span>
                    <span aria-hidden>·</span>
                    <span className="min-w-0 truncate group-open/hygiene:whitespace-normal group-open/hygiene:wrap-anywhere">
                      {first}
                    </span>
                  </summary>
                  {remaining.length > 0 && (
                    <ul className="my-1 pl-5">
                      {remaining.map((action) => (
                        <li key={action}>{action}</li>
                      ))}
                    </ul>
                  )}
                  <p className="m-0 text-xs text-muted">
                    External · data of {clockText(note.as_of, now)} {evidence}
                  </p>
                </details>
              );
            }
            return (
              <div key={note.plugin} data-plugin-note={note.plugin}>
                <h2 className="m-0 text-sm font-semibold">
                  {note.source} <span className="text-xs font-normal text-muted">external</span>
                </h2>
                <p className="m-0">
                  {note.summary ?? "No statement"}
                  {note.details.length > 0 && ` · ${note.details.join(" · ")}`}{" "}
                  <span className="text-xs text-muted">· data of {clockText(note.as_of, now)}</span>
                  {evidence}
                </p>
              </div>
            );
          })}
          <div>
            <h2 className="m-0 text-sm font-semibold">GitHub</h2>
            <p className="mt-0 mb-1">
              Bot rounds: {rounds} · {read.read_at ? `read ${clockText(read.read_at, now)}` : "not read yet"}
              {read.next_read_at && ` · next read ${clockText(read.next_read_at, now)}`}
            </p>
            <Tooltip tip="Read the threads, reviews and comments from GitHub again.">
              <button
                type="button"
                aria-disabled={busy || undefined}
                onClick={() => !busy && onRead()}
                className={buttonClass("plain", true)}
              >
                {busy ? "Reading GitHub…" : "Read GitHub again"}
              </button>
            </Tooltip>
          </div>
        </section>
      )}
    </div>
  );
}
