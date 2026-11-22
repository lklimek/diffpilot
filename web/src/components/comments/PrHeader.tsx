import type { PluginNote, PrBadgeView, WatchedPr, WatchState } from "../../api/generated/types";
import { clockText } from "../format/time";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { Tooltip } from "../tooltip/Tooltip";
import { StateBadge } from "./StateBadge";

const WATCH_TIP =
  "Watch: diffpilot checks this pull request for new comments and reviews itself, assesses them with its LLM at once and notifies you. Uses GitHub requests and LLM tokens.";

/** Words of a watch state (plan §2.8). */
export const WATCH_STATE: Record<WatchState, string> = {
  active: "Watching",
  waiting_for_bot_round: "Waiting for the bot's round to end",
  job_running: "Assessment running",
  limit_reached: "Not assessed automatically: today's limit",
  paused: "Watching paused: sign in again",
  ended: "Watching ended",
};

/** The latest automatic assessment, e.g. "Assessed automatically 09:12 · 5 questions". */
export function autoPassText(watch: Pick<WatchedPr, "last_auto_pass">, now: Date): string | null {
  const pass = watch.last_auto_pass;
  if (!pass) return null;
  return `Assessed automatically ${clockText(pass.at, now)} · ${pass.questions} ${pass.questions === 1 ? "question" : "questions"}`;
}

interface WatchToggleProps {
  /** The reader's watch; null when not watching. */
  watch: WatchedPr | null;
  busy: boolean;
  onChange: (watch: boolean) => void;
  now: Date;
}

/** The per-user Watch toggle of the PR page (US19) with its state line. */
export function WatchToggle({ watch, busy, onChange, now }: WatchToggleProps) {
  const on = watch !== null;
  const auto = watch ? autoPassText(watch, now) : null;
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Tooltip tip={busy ? `${WATCH_TIP} Not available while the current request runs.` : WATCH_TIP}>
        <button
          type="button"
          aria-pressed={on}
          aria-disabled={busy || undefined}
          onClick={() => !busy && onChange(!on)}
          className={`inline-flex min-h-6 cursor-pointer items-center gap-1.5 rounded-full border px-2.5 text-xs ${
            on ? "border-low-bd bg-low-bg font-semibold text-low" : "border-border bg-surface text-text"
          }`}
        >
          <span aria-hidden="true">{on ? "◉" : "○"}</span>
          {on ? "Watching" : "Watch"}
        </button>
      </Tooltip>
      {watch && (
        <span className="text-xs text-muted">
          {watch.state === "active"
            ? `Watching since ${clockText(watch.since, now)}`
            : (WATCH_STATE[watch.state] ?? watch.state)}
          {auto && ` · ${auto}`}
          {" · new comments are assessed automatically and use LLM tokens"}
        </span>
      )}
    </div>
  );
}

interface PrStatusStripProps {
  badge: PrBadgeView;
  /** Bot rounds so far. */
  rounds: number;
  /** Plug-in statements (PR Hygiene, thepastaclaw), attributed. */
  notes: PluginNote[];
  now: Date;
  /** Display id of the PR, for the evidence links' names. */
  itemId: string;
}

/** The PR's state badge, round counter and attributed plug-in statements (plan §3.1 header). */
export function PrStatusStrip({ badge, rounds, notes, now, itemId }: PrStatusStripProps) {
  return (
    <div
      role="group"
      aria-label="Pull request status"
      className="flex flex-wrap items-center gap-x-4 gap-y-1.5 rounded-lg border border-border bg-surface px-3.5 py-2 text-[13px]"
    >
      <StateBadge view={badge} />
      <span>
        <span className="text-muted">Bot rounds</span> <b className="tabular-nums">{rounds}</b>
      </span>
      {notes.map((note) => (
        <span key={note.plugin} data-plugin-note={note.plugin} className="inline-flex items-center gap-1">
          <span className="rounded bg-low-bg px-1 text-[11px] font-bold text-low">{note.source}</span>
          {note.summary ?? "no statement"}
          <span className="text-xs text-muted">· data of {clockText(note.as_of, now)}</span>
          <OpenExternal href={note.url} label={`${note.source} data for ${itemId}`} />
        </span>
      ))}
    </div>
  );
}
