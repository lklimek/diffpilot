import { Tooltip } from "../tooltip/Tooltip";
import type { PrRef, WatchedPr, WatchList } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText, dateTimeText } from "../format/time";
import { autoPassText, WATCH_STATE } from "./PrHeader";
import { StateBadge } from "./StateBadge";

const TH = "border-b border-border px-2 py-1.5 text-left text-[11px] font-semibold text-muted uppercase";
const TD = "border-b border-border px-2 py-1.5 align-top";
const name = (pr: PrRef) => `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;

interface WatchedListProps {
  list: WatchList;
  /** The PR page of a watched PR (in-app). */
  prHref: (pr: PrRef) => string;
  onNavigate: Navigate;
  onUnwatch: (pr: PrRef) => void;
  busy: boolean;
  now: Date;
}

/** V6 watched PRs (plan §2.8): since, last check, automatic assessment, state, Unwatch. */
export function WatchedList({ list, prHref, onNavigate, onUnwatch, busy, now }: WatchedListProps) {
  const active = list.watches.filter((w) => w.state !== "ended");
  const row = (w: WatchedPr) => {
    const ended = w.state === "ended";
    return (
      <tr key={name(w.pr)}>
        <td className={TD}>
          <a href={prHref(w.pr)} onClick={navigateOnClick(prHref(w.pr), onNavigate)}>
            {name(w.pr)}
          </a>{" "}
          <span className="text-muted">{w.title}</span>
        </td>
        <td className={TD}>{dateTimeText(w.since)}</td>
        <td className={TD}>{w.last_probe_at ? clockText(w.last_probe_at, now) : "—"}</td>
        <td className={TD}>{autoPassText(w, now) ?? "—"}</td>
        <td className={TD}>
          {ended ? (
            `${w.ended_at ? `Merged or closed ${dateTimeText(w.ended_at)}; ` : ""}watching ended`
          ) : (
            <span className="flex flex-wrap items-center gap-1.5">
              <StateBadge view={w.badge} />
              {w.state !== "active" && (
                <span className="text-xs text-muted">{WATCH_STATE[w.state] ?? w.state}</span>
              )}
            </span>
          )}
        </td>
        <td className={TD}>
          {!ended && (
            <Tooltip tip="Stops watching this pull request: new comments are no longer found and assessed automatically.">
              <button
                type="button"
                aria-label={`Unwatch ${name(w.pr)}`}
                aria-disabled={busy || undefined}
                onClick={() => !busy && onUnwatch(w.pr)}
                className={buttonClass("plain", true)}
              >
                Unwatch
              </button>
            </Tooltip>
          )}
        </td>
      </tr>
    );
  };
  return (
    <div>
      <p className="text-xs text-muted">
        {active.length} of {list.limit} watched. Checked every few minutes, and at once when PR Hygiene or
        thepastaclaw reports a change. New comments are assessed by diffpilot&apos;s LLM, one assessment per
        PR at a time, after the bot&apos;s round has ended; this uses GitHub requests and LLM tokens.
      </p>
      {list.watches.length === 0 ? (
        <p className="text-muted">You watch no pull request. Use Watch on a PR page.</p>
      ) : (
        <table className="w-full border-collapse text-[13px]">
          <caption className="sr-only">Watched pull requests</caption>
          <thead>
            <tr>
              {["Pull request", "Watching since", "Last check", "Automatic assessment", "State"].map((h) => (
                <th key={h} scope="col" className={TH}>
                  {h}
                </th>
              ))}
              <th scope="col" className={TH}>
                <span className="sr-only">Action</span>
              </th>
            </tr>
          </thead>
          <tbody>{list.watches.map(row)}</tbody>
        </table>
      )}
    </div>
  );
}
